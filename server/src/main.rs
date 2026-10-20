#![forbid(unsafe_code)]

mod api;
mod agent_control;
mod collection_settings;
mod config;
mod database;
mod guide;
mod insights;
mod operations;
mod performance;
mod network;
mod permission_reviews;

use anyhow::{Context, Result};
use api::AppState;
use axum_server::tls_rustls::RustlsConfig;
use config::ServerConfig;
use easymanage_protocol::PROTOCOL_VERSION;
use easymanage_runtime::privilege::ensure_administrator;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    initialize_crypto_provider();
    initialize_tracing();
    if let Err(error) = run().await {
        tracing::error!(error = %error, "server stopped with an error");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn initialize_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

async fn run() -> Result<()> {
    ensure_administrator("controller")?;
    easymanage_runtime::network::counter(easymanage_protocol::NetworkGroup::HttpBrowser);
    let config = Arc::new(ServerConfig::load()?);
    let pool = PgPoolOptions::new()
        .max_connections(config.database_max_connections)
        .acquire_timeout(Duration::from_secs(3))
        .connect_lazy_with(config.database_url.parse::<PgConnectOptions>()?
            .application_name("easymanage-server")
            .options([("statement_timeout", "30000"), ("idle_in_transaction_session_timeout", "30000")]));

    let tls = RustlsConfig::from_pem_file(&config.tls_certificate, &config.tls_private_key)
        .await
        .context("failed to load TLS certificate or private key")?;
    let bind_address = config.bind_address;
    let collection = collection_settings::CollectionSettings::defaults(config.report_interval_seconds);
    let initialized = database::initialize(pool.clone(), collection.clone());
    spawn_metrics_retention(pool.clone(), config.metrics_retention_days, initialized.clone());
    let performance_monitor = performance::start_sampler(pool.clone(), collection.clone(), config.web_root.clone(), initialized.clone());
    let app = api::router(AppState {
        pool: pool.clone(),
        config: Arc::clone(&config),
        live: Arc::new(RwLock::new(HashMap::new())),
        collection: collection.clone(),
    })
    .merge(collection_settings::router(
        pool.clone(),
        collection.clone(),
    ))
    .merge(operations::router(pool.clone(), initialized.clone(), config.clone()))
    .merge(permission_reviews::router(pool.clone(), collection.clone()))
    .merge(insights::router(
        pool.clone(),
        performance_monitor.clone(),
        collection,
    )?)
    .merge(network::router(pool.clone()))
    .merge(performance::router(pool, performance_monitor))
    .merge(guide::router(&config.web_root)?)
    .layer(axum::middleware::from_fn_with_state(initialized, database::guard))
    .layer(axum::middleware::from_fn(api::attach_request_id))
    .layer(axum::middleware::from_fn(network::account));

    info!(
        %bind_address,
        protocol_version = PROTOCOL_VERSION,
        web_root = %config.web_root.display(),
        "EasyManage server starting"
    );
    let handle = axum_server::Handle::<SocketAddr>::new();
    let shutdown_handle = handle.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        info!("shutdown signal received");
        shutdown_handle.graceful_shutdown(Some(Duration::from_secs(15)));
    });
    axum_server::bind_rustls(bind_address, tls)
        .handle(handle)
        .serve(app.into_make_service())
        .await
        .context("HTTPS server failed")?;
    Ok(())
}

fn spawn_metrics_retention(pool: PgPool, retention_days: i32, initialized: database::Initialized) {
    tokio::spawn(async move {
        database::wait_for_initialization(&initialized).await;
        let mut interval = tokio::time::interval(Duration::from_hours(6));
        loop {
            interval.tick().await;
            match sqlx::query(
                "DELETE FROM metrics_samples \
                 WHERE collected_at < NOW() - make_interval(days => $1)",
            )
            .bind(retention_days)
            .execute(&pool)
            .await
            {
                Ok(result) if result.rows_affected() > 0 => {
                    info!(rows = result.rows_affected(), "expired metrics removed");
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "metrics retention task failed"),
            }
            match sqlx::query(
                "DELETE FROM platform_process_samples \
                 WHERE sampled_at < NOW() - make_interval(days => $1)",
            )
            .bind(retention_days)
            .execute(&pool)
            .await
            {
                Ok(result) if result.rows_affected() > 0 => {
                    info!(
                        rows = result.rows_affected(),
                        "expired platform process samples removed"
                    );
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "platform process retention task failed"),
            }
            for table in ["server_storage_samples", "agent_storage_samples"] {
                let statement = format!(
                    "DELETE FROM {table} WHERE sampled_at < NOW() - make_interval(days => $1)"
                );
                if let Err(error) = sqlx::query(&statement)
                    .bind(retention_days)
                    .execute(&pool)
                    .await
                {
                    tracing::warn!(%error, %table, "platform storage retention task failed");
                }
            }
        }
    });
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

fn initialize_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("easymanage_server=info,tower_http=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .compact()
        .init();
}
