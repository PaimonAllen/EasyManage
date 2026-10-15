#![forbid(unsafe_code)]

// Register a real backend before enabling another target; do not build a fake
// "healthy" Agent which silently omits all OS-specific observations.
#[cfg(not(target_os = "linux"))]
compile_error!(
    "EasyManage Agent currently supports Linux only. Windows/macOS backends are not implemented; see docs/architecture.md."
);

#[cfg(target_os = "linux")]
mod app;
#[cfg(target_os = "linux")]
mod client;
#[cfg(target_os = "linux")]
mod config;
#[cfg(target_os = "linux")]
mod platform;
#[cfg(target_os = "linux")]
mod store;

#[cfg(target_os = "linux")]
use std::process::ExitCode;
#[cfg(target_os = "linux")]
use tracing_subscriber::EnvFilter;

// A monitoring daemon does not need one async worker per host CPU.
#[cfg(target_os = "linux")]
#[tokio::main(worker_threads = 2)]
async fn main() -> ExitCode {
    initialize_crypto_provider();
    initialize_tracing();
    if let Err(error) = app::run().await {
        tracing::error!(error = %error, "agent stopped with an error");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(target_os = "linux")]
fn initialize_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

#[cfg(target_os = "linux")]
fn initialize_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("easymanage_agent=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .compact()
        .init();
}
