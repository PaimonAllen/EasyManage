//! Platform-independent Agent scheduling, enrollment and durable reporting.
use crate::client::{ReportOutcome, ServerClient};
use crate::config::AgentConfig;
use crate::platform::{self, PlatformCollector};
use crate::store::Store;
use anyhow::{Context, Result};
use easymanage_protocol::{
    AGENT_VERSION, AgentControl, AgentLiveReport, AgentReport, CollectionPolicy, EnrollmentRequest,
    NodeInventory, PROTOCOL_VERSION,
};
use easymanage_runtime::shutdown::shutdown_signal;
use std::time::{Duration, Instant};
use tracing::{info, warn};
use uuid::Uuid;

pub(crate) async fn run() -> Result<()> {
    let config = AgentConfig::load()?;
    let store = Store::open(&config.state_database).await?;
    let client = ServerClient::new(&config)?;
    let node_id = store.node_id().await?;
    let mut token = store.agent_token().await?;
    let mut control = store.agent_control().await?;
    // Read persisted intent before any inventory, GPU or filesystem scanning.
    if let Some(token) = token.as_deref() {
        if let Err(error) = client.poll_control(token, node_id, &control).await {
            warn!(%error, "control unavailable; retaining persisted monitoring state");
        }
    }
    let mut collector: Option<platform::NativeCollector> = None;
    let mut policy = CollectionPolicy {
        users_enabled: Some(store.users_enabled().await?),
        permissions_enabled: Some(store.permissions_enabled().await?),
        proxy_enabled: Some(store.proxy_enabled().await?),
        report_seconds: config.report_interval.as_secs(),
        gpu_seconds: config.report_interval.as_secs(),
        ..Default::default()
    };
    policy.validate().map_err(anyhow::Error::msg)?;
    info!(%node_id, paused = control.paused, "EasyManage agent started; saved identity and privileges retained");
    let mut interval = tokio::time::interval(config.report_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut live_interval = tokio::time::interval(Duration::from_secs(1));
    live_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut control_interval = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(60),
        Duration::from_secs(60),
    );
    control_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut next_live_warning_at = Instant::now();
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        if let Some(next) = client.agent_control() {
            if next.revision > control.revision && next != control {
                store.save_agent_control(&next).await?;
                control = next;
                // Drop cancels collector tasks; no scan is scheduled while paused.
                if control.paused {
                    collector.take();
                }
                if let Some(token) = token.as_deref() {
                    if let Err(error) = client.poll_control(token, node_id, &control).await {
                        warn!(%error, "monitoring state saved; acknowledgement will retry");
                    }
                }
                control_interval.reset_after(Duration::from_secs(60));
                info!(
                    paused = control.paused,
                    revision = control.revision,
                    "monitoring state applied"
                );
            }
        }
        if let Some(mut next) = client.collection_policy() {
            // A legacy server cannot silently re-enable a locally disabled scan.
            next.users_enabled = next.users_enabled.or(policy.users_enabled);
            next.permissions_enabled = next.permissions_enabled.or(policy.permissions_enabled);
            next.proxy_enabled = next.proxy_enabled.or(policy.proxy_enabled);
            if next != policy {
                if next.users_enabled != policy.users_enabled {
                    store
                        .save_users_enabled(next.users_enabled.unwrap_or(true))
                        .await?;
                }
                if next.permissions_enabled != policy.permissions_enabled {
                    store
                        .save_permissions_enabled(next.permissions_enabled.unwrap_or(true))
                        .await?;
                }
                let report_changed = next.report_seconds != policy.report_seconds;
                if next.proxy_enabled != policy.proxy_enabled {
                    store
                        .save_proxy_enabled(next.proxy_enabled.unwrap_or(true))
                        .await?;
                }
                let live_changed = next.live_seconds != policy.live_seconds;
                policy = next;
                if let Some(collector) = collector.as_mut() {
                    PlatformCollector::apply_policy(collector, policy.clone());
                }
                let now = tokio::time::Instant::now();
                // No immediate extra probes or catch-up bursts when settings change.
                if report_changed {
                    interval = tokio::time::interval_at(
                        now + Duration::from_secs(policy.report_seconds),
                        Duration::from_secs(policy.report_seconds),
                    );
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                }
                if live_changed {
                    live_interval = tokio::time::interval_at(
                        now + Duration::from_secs(policy.live_seconds),
                        Duration::from_secs(policy.live_seconds),
                    );
                    live_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                }
                info!(
                    revision = policy.revision,
                    "collection policy applied without changing Agent privileges"
                );
            }
        }
        if !control.paused && collector.is_none() {
            let mut active = platform::initialize(&config.state_database, &store).await?;
            PlatformCollector::apply_policy(&mut active, policy.clone());
            collector = Some(active);
            interval.reset_immediately();
            live_interval.reset_after(Duration::from_secs(policy.live_seconds));
        }
        tokio::select! {
            _ = control_interval.tick(), if token.is_some() && control.paused => {
                if let Some(token) = token.as_deref() {
                    tokio::select! {
                        () = &mut shutdown => break,
                        result = client.poll_control(token, node_id, &control) => {
                            if let Err(error) = result { warn!(%error, "monitoring control unavailable; retaining current state"); }
                        }
                    }
                }
            }
            _ = interval.tick(), if !control.paused => {
                // Cancelling this future leaves the outbox intact and closes in-flight HTTP requests.
                tokio::select! {
                    () = &mut shutdown => break,
                    result = collect_and_send(collector.as_mut().expect("active collector"), &client, &store, &mut token, node_id, &control) => {
                        if let Err(error) = result {
                            warn!(%error, "metrics remain buffered for a later retry");
                        }
                    }
                }
            }
            _ = live_interval.tick(), if token.is_some() && !control.paused => {
                let report = AgentLiveReport {
                    protocol_version: PROTOCOL_VERSION,
                    node_id,
                    metrics: PlatformCollector::collect_fast(collector.as_mut().expect("active collector")),
                };
                if let Some(agent_token) = token.as_deref() {
                    let result = client.report_live(agent_token, &report).await;
                    if Instant::now() >= next_live_warning_at {
                        match result {
                            Ok(ReportOutcome::Unauthorized) => {
                                warn!("live metrics credentials rejected; full reports remain scheduled");
                                next_live_warning_at = Instant::now() + Duration::from_secs(30);
                            }
                            Err(error) => {
                                warn!(%error, "live metrics unavailable; full reports remain scheduled");
                                next_live_warning_at = Instant::now() + Duration::from_secs(30);
                            }
                            Ok(ReportOutcome::Accepted) => {}
                        }
                    }
                }
            }
            () = &mut shutdown => {
                info!("shutdown signal received");
                break;
            }
        }
    }
    Ok(())
}

async fn flush_outbox(client: &ServerClient, store: &Store, token: &str) -> Result<()> {
    // Bound catch-up work so an old backlog cannot starve fresh sampling or shutdown.
    for _ in 0..8 {
        if client.agent_control().is_some_and(|control| control.paused) {
            break;
        }
        let Some((id, report)) = store.oldest().await? else {
            break;
        };
        send_queued_report(client, store, token, id, &report).await?;
    }
    Ok(())
}

async fn collect_and_send(
    collector: &mut impl PlatformCollector,
    client: &ServerClient,
    store: &Store,
    token: &mut Option<String>,
    node_id: Uuid,
    control: &AgentControl,
) -> Result<()> {
    let inventory = collector.inventory()?;
    let mut metrics = collector.collect().await;
    metrics.network_activity = Some(easymanage_runtime::network::snapshot(true));
    metrics.agent_control = Some(control.clone());
    let report = AgentReport {
        reuse_software_inventory: false,
        protocol_version: PROTOCOL_VERSION,
        node_id,
        agent_version: AGENT_VERSION.to_owned(),
        inventory: inventory.clone(),
        metrics,
    };
    let latest_id = store.enqueue(&report).await?;
    if token.is_none() {
        *token = Some(enroll(client, store, node_id, &inventory).await?);
    }
    if let Some(token) = token.as_deref() {
        // Publish the current privilege/heartbeat immediately; a long offline backlog must
        // not hide a newly started basic/full agent. History remains in the durable outbox.
        send_queued_report(client, store, token, latest_id, &report).await?;
        tokio::time::timeout(
            std::time::Duration::from_secs(8),
            flush_outbox(client, store, token),
        )
        .await
        .context("upload batch deadline reached; unacknowledged metrics stay buffered")??;
        store.maintain_if_due().await;
    }
    Ok(())
}

async fn send_queued_report(
    client: &ServerClient,
    store: &Store,
    token: &str,
    id: i64,
    report: &AgentReport,
) -> Result<()> {
    match client.report(token, report).await? {
        ReportOutcome::Accepted => store.acknowledge(id).await,
        ReportOutcome::Unauthorized => anyhow::bail!(
            "agent credentials rejected; administrator must review enrollment (automatic re-enrollment is disabled)"
        ),
    }
}

async fn enroll(
    client: &ServerClient,
    store: &Store,
    node_id: Uuid,
    inventory: &NodeInventory,
) -> Result<String> {
    let response = client
        .enroll(&EnrollmentRequest {
            protocol_version: PROTOCOL_VERSION,
            requested_node_id: node_id,
            agent_version: AGENT_VERSION.to_owned(),
            inventory: inventory.clone(),
        })
        .await?;
    if response.node_id != node_id {
        anyhow::bail!("server returned an unexpected node id");
    }
    store.set_agent_token(&response.agent_token).await?;
    info!(%node_id, "agent enrollment completed");
    Ok(response.agent_token)
}
