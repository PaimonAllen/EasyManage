use crate::config::ServerConfig;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use easymanage_protocol::{
    AGENT_TOKEN_HEADER, AgentLiveReport, AgentReport, AgentReportResponse, ENROLLMENT_TOKEN_HEADER,
    EnrollmentRequest, EnrollmentResponse, ErrorResponse, GpuMetric, HealthResponse, LiveNode,
    LiveNodeListResponse, MetricsSnapshot, NodeSummary, PROTOCOL_VERSION,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::sync::RwLock;
use tower_http::compression::CompressionLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<ServerConfig>,
    pub live: Arc<RwLock<HashMap<Uuid, LiveNode>>>,
    pub collection: crate::collection_settings::CollectionSettings,
}

static LIVE_ACCESS: crate::read_cache::SnapshotCache<Arc<HashSet<Uuid>>> =
    crate::read_cache::SnapshotCache::new();

pub fn router(state: AppState) -> Router {
    let index = state.config.web_root.join("index.html");
    let static_files =
        ServeDir::new(&state.config.web_root).not_found_service(ServeFile::new(index.clone()));

    Router::new()
        .route_service("/usage-history", ServeFile::new(index.clone()))
        .route_service("/usage-history/", ServeFile::new(index))
        .route("/api/v1/health/live", get(live))
        .route("/api/v1/health/ready", get(ready))
        .route("/api/v1/nodes", get(list_nodes))
        .route("/api/v1/nodes/live", get(list_live_nodes))
        .route("/api/v1/nodes/{node_id}/history", get(node_history))
        .route("/api/v1/platform", get(platform))
        .route("/api/v1/agent/enroll", post(enroll_agent))
        .route("/api/v1/agent/report", post(report_metrics))
        .route("/api/v1/agent/live", post(report_live_metrics))
        .route("/api/v1/agent/control", post(agent_control))
        .fallback_service(static_files)
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn platform() -> Json<Value> {
    Json(json!({
        "documentation_url": "/guide/easymanage.html",
        "operations_enabled": true,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn live() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_owned(),
        service: "easymanage-server".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        timestamp: Utc::now(),
    })
}

async fn ready(State(state): State<AppState>) -> Result<Json<HealthResponse>, ApiError> {
    sqlx::query("SELECT 1")
        .execute(&state.pool)
        .await
        .map_err(ApiError::database)?;
    Ok(live().await)
}

async fn list_live_nodes(
    State(state): State<AppState>,
) -> Result<Json<LiveNodeListResponse>, ApiError> {
    let generated_at = Utc::now();
    let cutoff = generated_at
        - Duration::seconds(
            i64::try_from(state.collection.current().live_seconds + 3).unwrap_or(63),
        );
    let active = LIVE_ACCESS.get(async {
        sqlx::query_scalar("SELECT n.id FROM nodes n LEFT JOIN metrics_latest m ON m.node_id=n.id WHERE n.deleted_at IS NULL AND NOT n.monitoring_paused AND NOT EXISTS(SELECT 1 FROM agent_service_controls s WHERE s.node_id=n.id AND (s.desired_stopped OR s.state<>'running')) AND (n.monitoring_revision=0 OR (n.monitoring_applied_revision IS NULL AND m.metrics IS NOT NULL AND COALESCE(m.agent_control,'null'::jsonb)='null'::jsonb) OR m.agent_control=jsonb_build_object('revision',n.monitoring_revision,'paused',false))")
        .fetch_all(&state.pool)
        .await
        .map(|ids| Arc::new(ids.into_iter().collect::<HashSet<Uuid>>()))
        .map_err(ApiError::database)
    }, |ids| ids.capacity().saturating_mul(32)).await?;
    let nodes = state
        .live
        .read()
        .await
        .values()
        .filter(|node| node.received_at >= cutoff && active.contains(&node.node_id))
        .cloned()
        .collect();
    Ok(Json(LiveNodeListResponse {
        generated_at,
        nodes,
    }))
}

async fn report_live_metrics(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(report): Json<AgentLiveReport>,
) -> Result<StatusCode, ApiError> {
    verify_protocol(report.protocol_version)?;
    let provided_token = header_value(&headers, AGENT_TOKEN_HEADER)?;
    authenticate_agent(&state.pool, report.node_id, provided_token, true).await?;

    let metrics = &report.metrics;
    if !metrics.cpu_usage_percent.is_finite()
        || !(0.0..=100.0).contains(&metrics.cpu_usage_percent)
        || metrics.memory_total_bytes == 0
        || metrics.memory_used_bytes > metrics.memory_total_bytes
        || !metrics.network_received_bytes_per_second.is_finite()
        || metrics.network_received_bytes_per_second < 0.0
        || !metrics.network_transmitted_bytes_per_second.is_finite()
        || metrics.network_transmitted_bytes_per_second < 0.0
    {
        return Err(ApiError::bad_request("invalid live metrics"));
    }
    let now = Utc::now();
    let mut live = state.live.write().await;
    if live.len() > 1024 {
        let cutoff = now - Duration::minutes(1);
        live.retain(|_, node| node.received_at >= cutoff);
    }
    live.insert(
        report.node_id,
        LiveNode {
            node_id: report.node_id,
            received_at: now,
            metrics: report.metrics,
        },
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn enroll_agent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<EnrollmentRequest>,
) -> Result<Json<EnrollmentResponse>, ApiError> {
    let provided = header_value(&headers, ENROLLMENT_TOKEN_HEADER)?;
    verify_protocol(request.protocol_version)?;

    let node_id = request.requested_node_id;
    let agent_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let token_hash = hash_token(&agent_token);
    let now = Utc::now();
    let mut transaction = state.pool.begin().await.map_err(ApiError::database)?;

    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(node_id.to_string())
        .execute(&mut *transaction)
        .await
        .map_err(ApiError::database)?;
    let deleted: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM nodes WHERE id=$1 AND deleted_at IS NOT NULL)",
    )
    .bind(node_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(ApiError::database)?;
    if deleted {
        return Err(ApiError::unauthorized());
    }
    let bootstrap = sqlx::query("DELETE FROM machine_enrollments WHERE node_id=$1 AND token_hash=$2 AND expires_at>NOW() RETURNING expected_user")
        .bind(node_id).bind(hash_token(provided)).fetch_optional(&mut *transaction).await.map_err(ApiError::database)?;
    if bootstrap.is_none() {
        let pending: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM machine_enrollments WHERE node_id=$1)")
                .bind(node_id)
                .fetch_one(&mut *transaction)
                .await
                .map_err(ApiError::database)?;
        if pending {
            return Err(ApiError::unauthorized());
        }
        verify_header_secret(
            &headers,
            ENROLLMENT_TOKEN_HEADER,
            &state.config.enrollment_token,
        )?;
    }

    sqlx::query(
        r"
        INSERT INTO nodes (
            id, hostname, os_name, os_version, kernel_version, architecture, cpu_model,
            cpu_logical_cores, cpu_physical_cores, agent_version, last_seen
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        ON CONFLICT (id) DO UPDATE SET
            hostname = EXCLUDED.hostname,
            os_name = EXCLUDED.os_name,
            os_version = EXCLUDED.os_version,
            kernel_version = EXCLUDED.kernel_version,
            architecture = EXCLUDED.architecture,
            cpu_model = EXCLUDED.cpu_model,
            cpu_logical_cores = EXCLUDED.cpu_logical_cores,
            cpu_physical_cores = EXCLUDED.cpu_physical_cores,
            agent_version = EXCLUDED.agent_version,
            last_seen = EXCLUDED.last_seen,
            updated_at = NOW()
        ",
    )
    .bind(node_id)
    .bind(&request.inventory.hostname)
    .bind(&request.inventory.os_name)
    .bind(&request.inventory.os_version)
    .bind(&request.inventory.kernel_version)
    .bind(&request.inventory.architecture)
    .bind(&request.inventory.cpu_model)
    .bind(i64::from(request.inventory.cpu_logical_cores))
    .bind(request.inventory.cpu_physical_cores.map(i64::from))
    .bind(&request.agent_version)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(ApiError::database)?;

    sqlx::query(
        r"
        INSERT INTO agents (node_id, token_hash, enabled, enrolled_at)
        VALUES ($1, $2, TRUE, $3)
        ON CONFLICT (node_id) DO UPDATE SET
            token_hash = EXCLUDED.token_hash,
            enabled = TRUE,
            enrolled_at = EXCLUDED.enrolled_at
        ",
    )
    .bind(node_id)
    .bind(token_hash)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(ApiError::database)?;

    sqlx::query(
        r"
        INSERT INTO audit_logs (actor_type, actor_id, action, target_type, target_id, details)
        VALUES ('agent', $1, 'agent.enroll', 'node', $1, $2)
        ",
    )
    .bind(node_id.to_string())
    .bind(json!({ "hostname": request.inventory.hostname }))
    .execute(&mut *transaction)
    .await
    .map_err(ApiError::database)?;

    transaction.commit().await.map_err(ApiError::database)?;
    crate::agent_auth::invalidate();
    info!(%node_id, hostname = %request.inventory.hostname, "agent enrolled");

    Ok(Json(EnrollmentResponse {
        node_id,
        agent_token,
        report_interval_seconds: state.config.report_interval_seconds,
    }))
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep one report's atomic database transaction together"
)]
async fn report_metrics(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(report): Json<AgentReport>,
) -> Result<Json<AgentReportResponse>, ApiError> {
    if report.reuse_software_inventory {
        if report.protocol_version != easymanage_protocol::SOFTWARE_CACHE_PROTOCOL_VERSION {
            return Err(ApiError::bad_request("unsupported compact report protocol"));
        }
    } else { verify_protocol(report.protocol_version)?; }
    let provided_token = header_value(&headers, AGENT_TOKEN_HEADER)?;
    authenticate_agent(&state.pool, report.node_id, provided_token, false).await?;

    if report.reuse_software_inventory && !report.metrics.software_packages.is_empty() {
        return Err(ApiError::bad_request(
            "conflicting software inventory representation",
        ));
    }
    if let Some(process) = &report.metrics.platform_process {
        let rates = [
            process.cpu_percent_of_one_core,
            process.children_cpu_percent_of_one_core,
            process.logical_read_bytes_per_second,
            process.logical_written_bytes_per_second,
            process.disk_read_bytes_per_second,
            process.disk_written_bytes_per_second,
        ];
        if process.pid == 0
            || rates
                .into_iter()
                .flatten()
                .any(|value| !value.is_finite() || value < 0.0)
            || process
                .sample_interval_seconds
                .is_some_and(|value| !value.is_finite() || value <= 0.0 || value > 86400.0)
            || process
                .cpu_percent_of_host
                .is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value))
            || process.service.as_ref().is_some_and(|service| {
                service.unit.len() > 128
                    || !service.unit.starts_with("easymanage-")
                    || !service.unit.ends_with(".service")
                    || [
                        service.cpu_percent_of_one_core,
                        service.disk_read_bytes_per_second,
                        service.disk_written_bytes_per_second,
                    ]
                    .into_iter()
                    .flatten()
                    .any(|value| !value.is_finite() || value < 0.0)
            })
        {
            return Err(ApiError::bad_request("invalid resource accounting"));
        }
    }

    if let Some(activity) = &report.metrics.network_activity {
        let mut seen = std::collections::HashSet::new();
        if activity.process_id == 0
            || !(3..=4).contains(&activity.groups.len())
            || activity.started_at > activity.collected_at
            || activity.collected_at > report.metrics.collected_at + chrono::Duration::seconds(5)
            || activity
                .groups
                .iter()
                .any(|item| !item.group.is_agent() || !seen.insert(item.group as usize))
        {
            return Err(ApiError::bad_request("invalid Agent network accounting"));
        }
    }

    if report
        .metrics
        .hardware_details
        .as_ref()
        .is_some_and(|details| !details.valid())
    {
        return Err(ApiError::bad_request("hardware inventory exceeds limits"));
    }
    if let Some(process) = &report.metrics.platform_process {
        let valid_percent = process
            .cpu_percent_of_host
            .is_none_or(|value| value.is_finite() && (0.0..=100.0).contains(&value));
        let valid_rate =
            |value: Option<f64>| value.is_none_or(|value| value.is_finite() && value >= 0.0);
        if process.pid == 0
            || !valid_percent
            || !valid_rate(process.disk_read_bytes_per_second)
            || !valid_rate(process.disk_written_bytes_per_second)
            || process
                .resident_memory_bytes
                .is_some_and(|bytes| i64::try_from(bytes).is_err())
        {
            return Err(ApiError::bad_request("invalid platform process metrics"));
        }
    }
    if let Some(storage) = &report.metrics.platform_storage {
        let valid_size =
            |value: Option<u64>| value.is_none_or(|bytes| i64::try_from(bytes).is_ok());
        if !valid_size(storage.binary_bytes)
            || !valid_size(storage.state_bytes)
            || !valid_size(storage.total_bytes)
            || storage.web_bytes.is_some()
            || storage.database_bytes.is_some()
            || storage.total_bytes.is_some_and(|total| {
                storage
                    .binary_bytes
                    .zip(storage.state_bytes)
                    .and_then(|(binary, state)| binary.checked_add(state))
                    != Some(total)
            })
        {
            return Err(ApiError::bad_request("invalid Agent storage footprint"));
        }
    }

    if let Some(policy) = &report.metrics.collection_policy {
        policy.validate().map_err(ApiError::bad_request)?;
    }
    if report
        .metrics
        .agent_control
        .as_ref()
        .is_some_and(|control| control.revision > i64::MAX as u64)
    {
        return Err(ApiError::bad_request("invalid monitoring control revision"));
    }
    if let Some(frequency) = &report.metrics.cpu_frequency {
        if !frequency.valid() {
            return Err(ApiError::bad_request("invalid CPU frequency metrics"));
        }
    }
    let mut socket_ids = std::collections::HashSet::new();
    if report.metrics.cpu_sockets.len() > 64
        || report.metrics.cpu_sockets.iter().any(|cpu| {
            !socket_ids.insert(cpu.id)
                || cpu.model.len() > 1024
                || !(1..=4096).contains(&cpu.logical_cpus)
                || cpu
                    .physical_cores
                    .is_some_and(|cores| cores == 0 || cores > cpu.logical_cpus)
                || cpu
                    .usage_percent
                    .is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value))
                || cpu
                    .frequency
                    .as_ref()
                    .is_some_and(|frequency| !frequency.valid())
                || cpu
                    .temperature_celsius
                    .is_some_and(|value| !value.is_finite() || !(0.0..=150.0).contains(&value))
        })
    {
        return Err(ApiError::bad_request("invalid CPU socket metrics"));
    }
    if let Some(temperatures) = &report.metrics.temperatures {
        let valid = |value: f32| value.is_finite() && (0.0..=150.0).contains(&value);
        if temperatures.cpu_celsius.is_some_and(|value| !valid(value))
            || temperatures.ssds.len() > 128
            || temperatures.ssds.iter().any(|ssd| {
                ssd.device.is_empty() || ssd.device.len() > 64 || !valid(ssd.temperature_celsius)
            })
        {
            return Err(ApiError::bad_request("invalid temperature metrics"));
        }
    }

    let mut metrics_json = serde_json::to_value(&report.metrics)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    metrics_json
        .as_object_mut()
        .expect("snapshot object")
        .remove("software_packages");
    let network_received = report
        .metrics
        .networks
        .iter()
        .map(|metric| metric.received_bytes_per_second)
        .sum::<f64>();
    let network_transmitted = report
        .metrics
        .networks
        .iter()
        .map(|metric| metric.transmitted_bytes_per_second)
        .sum::<f64>();
    let (disk_read, disk_written) = if report.metrics.disk_collected_at.is_some() {
        aggregate_disk_io(&report.metrics.disks)
    } else {
        (None, None)
    };
    let gpu_json = serde_json::to_value(&report.metrics.gpus)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let memory_used = i64::try_from(report.metrics.memory_used_bytes)
        .map_err(|_| ApiError::bad_request("memory_used_bytes exceeds PostgreSQL BIGINT"))?;
    let now = Utc::now();
    let mut transaction = state.pool.begin().await.map_err(ApiError::database)?;

    if report.reuse_software_inventory {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM node_software_inventory WHERE node_id=$1)",
        )
        .bind(report.node_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(ApiError::database)?;
        if !exists {
            return Err(ApiError {
                status: StatusCode::CONFLICT,
                message: "software inventory required".into(),
            });
        }
    } else {
        sqlx::query("INSERT INTO node_software_inventory(node_id,collected_at,packages) VALUES($1,$2,$3) ON CONFLICT(node_id) DO UPDATE SET collected_at=EXCLUDED.collected_at,packages=CASE WHEN node_software_inventory.packages IS DISTINCT FROM EXCLUDED.packages THEN EXCLUDED.packages ELSE node_software_inventory.packages END WHERE EXCLUDED.collected_at>node_software_inventory.collected_at OR (EXCLUDED.collected_at=node_software_inventory.collected_at AND node_software_inventory.packages IS DISTINCT FROM EXCLUDED.packages)")
            .bind(report.node_id).bind(report.metrics.collected_at)
            .bind(serde_json::to_value(&report.metrics.software_packages).map_err(|error|ApiError::internal(error.to_string()))?)
            .execute(&mut *transaction).await.map_err(ApiError::database)?;
    }

    let updated_node = sqlx::query(
        r"
        UPDATE nodes SET
            hostname = $2, os_name = $3, os_version = $4, kernel_version = $5,
            architecture = $6, cpu_model = $7, cpu_logical_cores = $8,
            cpu_physical_cores = $9, agent_version = $10, last_seen = $11,
            updated_at = NOW()
        WHERE id = $1 AND NOT EXISTS (
            SELECT 1 FROM metrics_latest
            WHERE node_id = $1 AND collected_at > $12
        )
        ",
    )
    .bind(report.node_id)
    .bind(&report.inventory.hostname)
    .bind(&report.inventory.os_name)
    .bind(&report.inventory.os_version)
    .bind(&report.inventory.kernel_version)
    .bind(&report.inventory.architecture)
    .bind(&report.inventory.cpu_model)
    .bind(i64::from(report.inventory.cpu_logical_cores))
    .bind(report.inventory.cpu_physical_cores.map(i64::from))
    .bind(&report.agent_version)
    .bind(now)
    .bind(report.metrics.collected_at)
    .execute(&mut *transaction)
    .await
    .map_err(ApiError::database)?;

    // Historical catch-up still proves connectivity, but must not roll back inventory/version.
    if updated_node.rows_affected() == 0 {
        sqlx::query("UPDATE nodes SET last_seen = $2 WHERE id = $1")
            .bind(report.node_id)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(ApiError::database)?;
    }

    sqlx::query(
        r"
        INSERT INTO metrics_latest (node_id, collected_at, metrics, updated_at,agent_control)
        VALUES ($1, $2, $3, NOW(),$4)
        ON CONFLICT (node_id) DO UPDATE SET
            collected_at = EXCLUDED.collected_at,
            metrics = EXCLUDED.metrics,
            agent_control = EXCLUDED.agent_control,
            updated_at = NOW()
        WHERE EXCLUDED.collected_at >= metrics_latest.collected_at
        ",
    )
    .bind(report.node_id)
    .bind(report.metrics.collected_at)
    .bind(metrics_json)
    .bind(report.metrics.agent_control.as_ref().map(serde_json::to_value).transpose().map_err(|error|ApiError::internal(error.to_string()))?)
    .execute(&mut *transaction)
    .await
    .map_err(ApiError::database)?;

    crate::permission_reviews::invalidate_changed(&mut transaction, report.node_id)
        .await
        .map_err(ApiError::database)?;

    sqlx::query(
        r"
        INSERT INTO metrics_samples (
            node_id, collected_at, cpu_usage_percent, memory_used_bytes,
            network_received_bytes_per_second, network_transmitted_bytes_per_second, gpu_metrics,
            disk_collected_at, disk_read_bytes_per_second, disk_written_bytes_per_second,
            agent_pid, agent_cpu_percent_of_host, agent_resident_memory_bytes,
            agent_disk_read_bytes_per_second, agent_disk_written_bytes_per_second, gpu_collected_at, fast_collected_at, temperatures,
            agent_sample_interval_seconds,agent_cpu_percent_of_one_core,agent_service_cpu_percent_of_one_core,agent_service_memory_current_bytes
        ) SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18,$19,$20,$21,$22
          WHERE NOT EXISTS(SELECT 1 FROM metrics_samples WHERE node_id=$1 AND collected_at=$2)
        ",
    )
    .bind(report.node_id)
    .bind(report.metrics.collected_at)
    .bind(f64::from(report.metrics.cpu_usage_percent))
    .bind(memory_used)
    .bind(network_received)
    .bind(network_transmitted)
    .bind(gpu_json)
    .bind(report.metrics.disk_collected_at)
    .bind(disk_read)
    .bind(disk_written)
    .bind(
        report
            .metrics
            .platform_process
            .as_ref()
            .map(|process| i64::from(process.pid)),
    )
    .bind(
        report
            .metrics
            .platform_process
            .as_ref()
            .and_then(|process| process.cpu_percent_of_host),
    )
    .bind(
        report
            .metrics
            .platform_process
            .as_ref()
            .and_then(|process| process.resident_memory_bytes)
            .and_then(|bytes| i64::try_from(bytes).ok()),
    )
    .bind(
        report
            .metrics
            .platform_process
            .as_ref()
            .and_then(|process| process.disk_read_bytes_per_second),
    )
    .bind(
        report
            .metrics
            .platform_process
            .as_ref()
            .and_then(|process| process.disk_written_bytes_per_second),
    )
    .bind(report.metrics.gpu_collected_at)
    .bind(report.metrics.fast_collected_at)
    .bind(report.metrics.temperatures.as_ref().map(serde_json::to_value).transpose()
        .map_err(|error| ApiError::internal(error.to_string()))?)
    .bind(report.metrics.platform_process.as_ref().and_then(|metric|metric.sample_interval_seconds))
    .bind(report.metrics.platform_process.as_ref().and_then(|metric|metric.cpu_percent_of_one_core))
    .bind(report.metrics.platform_process.as_ref().and_then(|metric|metric.service.as_ref()).and_then(|service|service.cpu_percent_of_one_core))
    .bind(report.metrics.platform_process.as_ref().and_then(|metric|metric.service.as_ref()).and_then(|service|service.memory_current_bytes).and_then(|value|i64::try_from(value).ok()))
    .execute(&mut *transaction)
    .await
    .map_err(ApiError::database)?;

    if let Some(storage) = &report.metrics.platform_storage
        && storage.collected_at >= now - Duration::days(30)
        && storage.collected_at <= now + Duration::minutes(5)
    {
        sqlx::query(
            "INSERT INTO agent_storage_samples (node_id, sampled_at, binary_bytes, state_bytes, total_bytes) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (node_id, sampled_at) DO NOTHING",
        )
        .bind(report.node_id)
        .bind(storage.collected_at)
        .bind(storage.binary_bytes.and_then(|bytes| i64::try_from(bytes).ok()))
        .bind(storage.state_bytes.and_then(|bytes| i64::try_from(bytes).ok()))
        .bind(storage.total_bytes.and_then(|bytes| i64::try_from(bytes).ok()))
        .execute(&mut *transaction)
        .await
        .map_err(ApiError::database)?;
    }

    sqlx::query("UPDATE agents SET last_authenticated_at = $2 WHERE node_id = $1")
        .bind(report.node_id)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(ApiError::database)?;

    let agent_control = if report.metrics.agent_control.is_some() {
        Some(
            crate::agent_control::desired(
                &mut transaction,
                report.node_id,
                report.metrics.agent_control.as_ref(),
            )
            .await
            .map_err(ApiError::database)?,
        )
    } else {
        None
    };
    transaction.commit().await.map_err(ApiError::database)?;
    crate::read_cache::invalidate();
    Ok(Json(AgentReportResponse {
        software_inventory_cached: Some(true),
        agent_control,
        accepted_at: now,
        collection_policy: Some(
            state.collection.for_node_with_proxy(
                report.node_id,
                report
                    .metrics
                    .collection_policy
                    .as_ref()
                    .is_some_and(|p| p.users_enabled.is_some()),
                report
                    .metrics
                    .collection_policy
                    .as_ref()
                    .is_some_and(|p| p.permissions_enabled.is_some()),
                report
                    .metrics
                    .collection_policy
                    .as_ref()
                    .is_some_and(|p| p.proxy_enabled.is_some()),
            ),
        ),
    }))
}

#[derive(serde::Deserialize)]
struct NodeView {
    include_software: Option<bool>,
}

#[derive(Clone)]
struct NodeViewBody {
    etag: HeaderValue,
    body: Bytes,
}
static NODE_VIEWS: [crate::read_cache::SnapshotCache<NodeViewBody>; 2] = [
    crate::read_cache::SnapshotCache::new(),
    crate::read_cache::SnapshotCache::new(),
];

async fn list_nodes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(view): Query<NodeView>,
) -> Result<Response, ApiError> {
    let software = view.include_software.unwrap_or(true);
    let cached = NODE_VIEWS[usize::from(software)].get(
        load_node_view(&state, software), |view| view.body.len()
    ).await?;
    let unchanged = headers
        .get("if-none-match")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|value| value.trim().as_bytes() == cached.etag.as_bytes()));
    let mut response = if unchanged {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        Response::new(Body::from(cached.body))
    };
    response.headers_mut().insert("etag", cached.etag);
    response.headers_mut().insert(
        "cache-control", HeaderValue::from_static("private, no-cache"),
    );
    response.headers_mut().insert(
        "content-type", HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

async fn load_node_view(state: &AppState, software: bool) -> Result<NodeViewBody, ApiError> {
    let rows = sqlx::query(
        r"
        SELECT n.id, n.monitoring_paused, n.monitoring_revision, n.monitoring_applied_revision, s.state AS agent_service_state,s.desired_stopped AS agent_service_desired_stopped,s.revision AS agent_service_revision, COALESCE(NULLIF(n.display_name,''),n.hostname) AS hostname, n.os_name, n.os_version, n.kernel_version, n.architecture,
               n.cpu_model, n.cpu_logical_cores, n.cpu_physical_cores, n.agent_version,
               n.last_seen, CASE WHEN $1 AND m.metrics IS NOT NULL THEN m.metrics || jsonb_build_object('software_packages',COALESCE(p.packages,'[]'::jsonb)) ELSE m.metrics END AS metrics
        FROM nodes n
        LEFT JOIN metrics_latest m ON m.node_id = n.id
        LEFT JOIN node_software_inventory p ON p.node_id = n.id
        LEFT JOIN agent_service_controls s ON s.node_id=n.id
        WHERE n.deleted_at IS NULL
        ORDER BY n.hostname, n.id
        ",
    )
    .bind(software)
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::database)?;

    let online_cutoff = Utc::now() - Duration::seconds(state.config.online_window_seconds);
    let nodes = rows
        .iter()
        .map(|row| map_node(row, online_cutoff))
        .collect::<Result<Vec<_>, _>>()?;
    let bytes =
        serde_json::to_vec(&nodes).map_err(|error| ApiError::internal(error.to_string()))?;
    let etag = format!("W/\"{}\"", hex::encode(Sha256::digest(&bytes)));
    let mut body = b"{\"generated_at\":".to_vec();
    serde_json::to_writer(&mut body, &Utc::now())
        .map_err(|error| ApiError::internal(error.to_string()))?;
    body.extend_from_slice(b",\"nodes\":");
    body.extend_from_slice(&bytes);
    body.push(b'}');
    Ok(NodeViewBody {
        etag: HeaderValue::from_str(&etag).map_err(|error| ApiError::internal(error.to_string()))?,
        body: Bytes::from(body),
    })
}

async fn agent_control(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<easymanage_protocol::AgentControlRequest>,
) -> Result<Json<easymanage_protocol::AgentControlResponse>, ApiError> {
    verify_control_revision(&request.applied)?;
    let token = header_value(&headers, AGENT_TOKEN_HEADER)?;
    authenticate_agent(&state.pool, request.node_id, token, false).await?;
    let mut tx = state.pool.begin().await.map_err(ApiError::database)?;
    let control = crate::agent_control::desired(&mut tx, request.node_id, Some(&request.applied))
        .await
        .map_err(ApiError::database)?;
    tx.commit().await.map_err(ApiError::database)?;
    crate::read_cache::invalidate();
    Ok(Json(easymanage_protocol::AgentControlResponse {
        control,
        collection_policy: state.collection.for_node_with_proxy(
            request.node_id,
            true,
            true,
            request.supports_proxy_collection,
        ),
    }))
}
fn verify_control_revision(control: &easymanage_protocol::AgentControl) -> Result<(), ApiError> {
    if control.revision > i64::MAX as u64 {
        return Err(ApiError::bad_request("invalid monitoring control revision"));
    }
    Ok(())
}

#[derive(Serialize)]
struct HistoryGpu {
    index: u32,
    utilization_percent: f32,
    memory_used_bytes: u64,
    memory_total_bytes: u64,
    temperature_celsius: Option<f32>,
}

#[derive(Serialize)]
struct HistoryPoint {
    temperatures: Option<easymanage_protocol::HostTemperatures>,
    collected_at: DateTime<Utc>,
    gpu_collected_at: Option<DateTime<Utc>>,
    fast_collected_at: Option<DateTime<Utc>>,
    cpu_usage_percent: f64,
    memory_used_bytes: u64,
    network_received_bytes_per_second: f64,
    network_transmitted_bytes_per_second: f64,
    disk_collected_at: Option<DateTime<Utc>>,
    disk_read_bytes_per_second: Option<f64>,
    disk_written_bytes_per_second: Option<f64>,
    gpus: Vec<HistoryGpu>,
}

#[derive(Serialize)]
struct NodeHistoryResponse {
    node_id: Uuid,
    generated_at: DateTime<Utc>,
    window_seconds: u32,
    points: Vec<HistoryPoint>,
}

async fn node_history(
    State(state): State<AppState>,
    Path(node_id): Path<Uuid>,
) -> Result<Json<NodeHistoryResponse>, ApiError> {
    const WINDOW_SECONDS: u32 = 10 * 60;
    let generated_at = Utc::now();
    let rows = sqlx::query(
        "SELECT collected_at, cpu_usage_percent, memory_used_bytes, \
         network_received_bytes_per_second, network_transmitted_bytes_per_second, gpu_metrics, \
         disk_collected_at, disk_read_bytes_per_second, disk_written_bytes_per_second, gpu_collected_at, fast_collected_at, temperatures \
         FROM metrics_samples WHERE node_id = $1 AND collected_at >= $2 \
         ORDER BY collected_at DESC, id DESC LIMIT 180",
    )
    .bind(node_id)
    .bind(generated_at - Duration::seconds(i64::from(WINDOW_SECONDS)))
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::database)?;
    let mut points = Vec::with_capacity(rows.len());
    for row in rows {
        let collected_at: DateTime<Utc> =
            row.try_get("collected_at").map_err(ApiError::database)?;
        if points
            .last()
            .is_some_and(|point: &HistoryPoint| point.collected_at == collected_at)
        {
            continue;
        }
        let gpus: Vec<GpuMetric> =
            serde_json::from_value(row.try_get("gpu_metrics").map_err(ApiError::database)?)
                .map_err(|error| ApiError::internal(error.to_string()))?;
        let memory_used_bytes: i64 = row
            .try_get("memory_used_bytes")
            .map_err(ApiError::database)?;
        points.push(HistoryPoint {
            temperatures: row
                .try_get::<Option<serde_json::Value>, _>("temperatures")
                .map_err(ApiError::database)?
                .map(serde_json::from_value)
                .transpose()
                .map_err(|error| ApiError::internal(error.to_string()))?,
            collected_at,
            gpu_collected_at: row
                .try_get("gpu_collected_at")
                .map_err(ApiError::database)?,
            fast_collected_at: row
                .try_get("fast_collected_at")
                .map_err(ApiError::database)?,
            cpu_usage_percent: row
                .try_get("cpu_usage_percent")
                .map_err(ApiError::database)?,
            memory_used_bytes: memory_used_bytes
                .try_into()
                .map_err(|_| ApiError::internal("negative historical memory usage"))?,
            network_received_bytes_per_second: row
                .try_get("network_received_bytes_per_second")
                .map_err(ApiError::database)?,
            network_transmitted_bytes_per_second: row
                .try_get("network_transmitted_bytes_per_second")
                .map_err(ApiError::database)?,
            disk_collected_at: row
                .try_get("disk_collected_at")
                .map_err(ApiError::database)?,
            disk_read_bytes_per_second: row
                .try_get("disk_read_bytes_per_second")
                .map_err(ApiError::database)?,
            disk_written_bytes_per_second: row
                .try_get("disk_written_bytes_per_second")
                .map_err(ApiError::database)?,
            gpus: gpus
                .into_iter()
                .map(|gpu| HistoryGpu {
                    index: gpu.index,
                    utilization_percent: gpu.utilization_percent,
                    memory_used_bytes: gpu.memory_used_bytes,
                    memory_total_bytes: gpu.memory_total_bytes,
                    temperature_celsius: gpu.temperature_celsius,
                })
                .collect(),
        });
    }
    points.reverse();
    Ok(Json(NodeHistoryResponse {
        node_id,
        generated_at,
        window_seconds: WINDOW_SECONDS,
        points,
    }))
}

fn aggregate_disk_io(disks: &[easymanage_protocol::DiskMetric]) -> (Option<f64>, Option<f64>) {
    let mut seen = HashSet::new();
    let mut read = None;
    let mut written = None;
    for disk in disks {
        // Several mount points can refer to the same block device. Count its I/O once.
        if !seen.insert(disk.name.as_str()) {
            continue;
        }
        if let Some(rate) = disk
            .read_bytes_per_second
            .filter(|rate| rate.is_finite() && *rate >= 0.0)
        {
            *read.get_or_insert(0.0) += rate;
        }
        if let Some(rate) = disk
            .written_bytes_per_second
            .filter(|rate| rate.is_finite() && *rate >= 0.0)
        {
            *written.get_or_insert(0.0) += rate;
        }
    }
    (read, written)
}

fn monitoring_report_current(
    revision: u64,
    applied: Option<u64>,
    control: Option<&easymanage_protocol::AgentControl>,
) -> bool {
    match control {
        Some(control) => control.revision == revision && !control.paused,
        // Legacy collectors cannot acknowledge pause/resume. Their real, fresh
        // reports remain usable once the user has explicitly resumed monitoring.
        // Never use this fallback after a capable Agent has acknowledged control.
        None => applied.is_none(),
    }
}

fn map_node(row: &PgRow, online_cutoff: chrono::DateTime<Utc>) -> Result<NodeSummary, ApiError> {
    let monitoring_paused: bool = row
        .try_get("monitoring_paused")
        .map_err(ApiError::database)?;
    let monitoring_revision = row
        .try_get::<i64, _>("monitoring_revision")
        .map_err(ApiError::database)? as u64;
    let monitoring_applied_revision = row
        .try_get::<Option<i64>, _>("monitoring_applied_revision")
        .map_err(ApiError::database)?
        .map(|revision| revision as u64);
    let last_seen = row.try_get("last_seen").map_err(ApiError::database)?;
    let metrics = row
        .try_get::<Option<Value>, _>("metrics")
        .map_err(ApiError::database)?
        .map(serde_json::from_value::<MetricsSnapshot>)
        .transpose()
        .map_err(|error| ApiError::internal(error.to_string()))?;

    Ok(NodeSummary {
        agent_service_state: row.try_get("agent_service_state").map_err(ApiError::database)?,
        agent_service_desired_stopped: row.try_get::<Option<bool>,_>("agent_service_desired_stopped").map_err(ApiError::database)?.unwrap_or(false),
        agent_service_revision: row.try_get::<Option<i64>,_>("agent_service_revision").map_err(ApiError::database)?.unwrap_or(0) as u64,
        monitoring_paused,
        monitoring_revision,
        monitoring_applied_revision,
        id: row.try_get("id").map_err(ApiError::database)?,
        hostname: row.try_get("hostname").map_err(ApiError::database)?,
        os_name: row.try_get("os_name").map_err(ApiError::database)?,
        os_version: row.try_get("os_version").map_err(ApiError::database)?,
        kernel_version: row.try_get("kernel_version").map_err(ApiError::database)?,
        architecture: row.try_get("architecture").map_err(ApiError::database)?,
        cpu_model: row.try_get("cpu_model").map_err(ApiError::database)?,
        cpu_logical_cores: row
            .try_get::<i32, _>("cpu_logical_cores")
            .map_err(ApiError::database)?
            .try_into()
            .map_err(|_| ApiError::internal("invalid cpu_logical_cores"))?,
        cpu_physical_cores: row
            .try_get::<Option<i32>, _>("cpu_physical_cores")
            .map_err(ApiError::database)?
            .map(u32::try_from)
            .transpose()
            .map_err(|_| ApiError::internal("invalid cpu_physical_cores"))?,
        agent_version: row.try_get("agent_version").map_err(ApiError::database)?,
        last_seen,
        online: !row.try_get::<Option<bool>,_>("agent_service_desired_stopped").map_err(ApiError::database)?.unwrap_or(false)
            && row.try_get::<Option<String>,_>("agent_service_state").map_err(ApiError::database)?.is_none_or(|state|state=="running")
            && !monitoring_paused
            && last_seen >= online_cutoff
            && (monitoring_revision == 0
                || metrics.as_ref().is_some_and(|snapshot| {
                    monitoring_report_current(
                        monitoring_revision,
                        monitoring_applied_revision,
                        snapshot.agent_control.as_ref(),
                    )
                })),
        metrics,
    })
}

#[cfg(test)]
mod monitoring_compatibility_tests {
    use super::monitoring_report_current;
    use easymanage_protocol::AgentControl;

    #[test]
    fn legacy_resume_does_not_require_an_impossible_control_acknowledgement() {
        assert!(monitoring_report_current(2, None, None));
        assert!(!monitoring_report_current(2, Some(1), None));
        assert!(!monitoring_report_current(2, Some(2), None));
        assert!(!monitoring_report_current(
            2,
            None,
            Some(&AgentControl {
                revision: 0,
                paused: false
            })
        ));
        assert!(!monitoring_report_current(
            2,
            Some(2),
            Some(&AgentControl {
                revision: 1,
                paused: true
            })
        ));
        assert!(monitoring_report_current(
            2,
            Some(2),
            Some(&AgentControl {
                revision: 2,
                paused: false
            })
        ));
    }
}

async fn authenticate_agent(
    pool: &PgPool,
    node_id: Uuid,
    token: &str,
    live_lane: bool,
) -> Result<(), ApiError> {
    let provided_hash = hash_token(token);
    if live_lane && crate::agent_auth::accepts(node_id, &provided_hash) {
        return Ok(());
    }
    let epoch = crate::agent_auth::generation();
    let expected_hash = sqlx::query_scalar::<_, String>(
        "SELECT a.token_hash FROM agents a JOIN nodes n ON n.id=a.node_id WHERE a.node_id = $1 AND a.enabled = TRUE AND n.deleted_at IS NULL",
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::database)?
    .ok_or_else(ApiError::unauthorized)?;
    if constant_time_equal(expected_hash.trim(), &provided_hash) {
        crate::agent_auth::remember(node_id, provided_hash, epoch);
        Ok(())
    } else {
        warn!(%node_id, "agent authentication failed");
        Err(ApiError::unauthorized())
    }
}

fn verify_protocol(version: u16) -> Result<(), ApiError> {
    if version == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(ApiError::bad_request("unsupported protocol version"))
    }
}

fn verify_header_secret(
    headers: &HeaderMap,
    header_name: &str,
    expected: &str,
) -> Result<(), ApiError> {
    let provided = header_value(headers, header_name)?;
    if constant_time_equal(provided, expected) {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}

fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, ApiError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(ApiError::unauthorized)
}

pub(crate) fn constant_time_equal(left: &str, right: &str) -> bool {
    left.as_bytes().ct_eq(right.as_bytes()).into()
}

pub(crate) fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: "authentication failed".to_owned(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    fn database(error: sqlx::Error) -> Self {
        tracing::error!(%error, "database operation failed");
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "数据库暂不可用，请检查 PostgreSQL；Agent 管理中的强制重启仍可使用".into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = Json(ErrorResponse {
            error: self.message,
        })
        .into_response();
        *response.status_mut() = self.status;
        response
    }
}

pub async fn attach_request_id(
    mut request: Request<Body>,
    next: axum::middleware::Next,
) -> Response {
    let documentation =
        request.uri().path() == "/guide" || request.uri().path().starts_with("/guide/");
    let request_id = request
        .headers()
        .get("x-request-id")
        .cloned()
        .unwrap_or_else(|| {
            HeaderValue::from_str(&Uuid::new_v4().to_string())
                .unwrap_or_else(|_| HeaderValue::from_static("generated-request-id"))
        });
    request
        .headers_mut()
        .insert("x-request-id", request_id.clone());

    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("x-request-id", request_id);
    headers.insert(
        "strict-transport-security",
        HeaderValue::from_static("max-age=31536000"),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "x-frame-options",
        HeaderValue::from_static(if documentation { "SAMEORIGIN" } else { "DENY" }),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static(if documentation {
            // VitePress emits small inline bootstrap scripts in its trusted, repository-built HTML.
            "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; \
             img-src 'self'; connect-src 'self'; frame-src 'self'; object-src 'none'; base-uri 'none'; \
             frame-ancestors 'self'"
        } else {
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
             img-src 'self'; connect-src 'self'; frame-src 'self'; object-src 'none'; base-uri 'none'; \
             frame-ancestors 'none'"
        }),
    );
    response
}
