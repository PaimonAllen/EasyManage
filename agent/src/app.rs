//! Platform-independent Agent scheduling, enrollment and durable reporting.
use crate::client::{ReportOutcome, ServerClient};
use crate::config::AgentConfig;
use crate::platform::{self, PlatformCollector};
use crate::store::Store;
use anyhow::{Context, Result};
use easymanage_protocol::{
    AGENT_VERSION, AgentLiveReport, AgentReport, CollectionPolicy, EnrollmentRequest,
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
    let mut collector = platform::initialize(&config.state_database, &store).await?;
    let mut policy = CollectionPolicy {
        report_seconds: config.report_interval.as_secs(),
        gpu_seconds: config.report_interval.as_secs(),
        ..Default::default()
    };
    policy.validate().map_err(anyhow::Error::msg)?;
    collector.apply_policy(policy.clone());
    let inventory = collector.inventory()?;
    let node_id = store.node_id().await?;
    let mut token = store.agent_token().await?;

    info!(%node_id, hostname = %inventory.hostname, effective_uid = ?platform::effective_uid(), "EasyManage agent started; privileges reflect its actual execution user");
    let mut interval = tokio::time::interval(config.report_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut live_interval = tokio::time::interval(Duration::from_secs(1));
    live_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut next_live_warning_at = Instant::now();
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        if let Some(next) = client.collection_policy().filter(|next| next != &policy) {
            let report_changed = next.report_seconds != policy.report_seconds;
            let live_changed = next.live_seconds != policy.live_seconds;
            policy = next;
            collector.apply_policy(policy.clone());
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
        tokio::select! {
            _ = interval.tick() => {
                // Cancelling this future leaves the outbox intact and closes in-flight HTTP requests.
                tokio::select! {
                    () = &mut shutdown => break,
                    result = collect_and_send(&mut collector, &client, &store, &mut token, node_id) => {
                        if let Err(error) = result {
                            warn!(%error, "metrics remain buffered for a later retry");
                        }
                    }
                }
            }
            _ = live_interval.tick(), if token.is_some() => {
                let report = AgentLiveReport {
                    protocol_version: PROTOCOL_VERSION,
                    node_id,
                    metrics: collector.collect_fast(),
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
) -> Result<()> {
    let inventory = collector.inventory()?;
    let report = AgentReport {
        protocol_version: PROTOCOL_VERSION,
        node_id,
        agent_version: AGENT_VERSION.to_owned(),
        inventory: inventory.clone(),
        metrics: collector.collect().await,
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
