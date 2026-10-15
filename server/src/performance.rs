//! Process-only accounting for the EasyManage controller and every registered Agent.
//! The controller sampler runs independently of page views, so health-page work is included.
use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use chrono::{DateTime, Utc};
use easymanage_protocol::{PlatformProcessMetric, PlatformStorageMetric};
use easymanage_runtime::process::ProcessSampler;
use easymanage_runtime::storage::{bounded_tree_bytes, executable_bytes};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Row, postgres::PgRow};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

const SUMMARY_CACHE: Duration = Duration::from_secs(60);
const STORAGE_SAMPLE_INTERVAL: Duration = Duration::from_secs(3_600);

#[derive(Clone)]
pub struct PerformanceMonitor {
    current: Arc<RwLock<Option<ProcessSnapshot>>>,
    storage: Arc<RwLock<Option<PlatformStorageMetric>>>,
}

#[derive(Clone)]
pub struct ProcessSnapshot {
    pub sampled_at: DateTime<Utc>,
    pub metric: PlatformProcessMetric,
}

impl PerformanceMonitor {
    pub async fn current(&self) -> Option<ProcessSnapshot> {
        self.current.read().await.clone()
    }
    pub async fn current_storage(&self) -> Option<PlatformStorageMetric> {
        self.storage.read().await.clone()
    }
}

pub fn start_sampler(
    pool: PgPool,
    settings: crate::collection_settings::CollectionSettings,
    web_root: PathBuf,
    initialized: crate::database::Initialized,
) -> PerformanceMonitor {
    let monitor = PerformanceMonitor {
        current: Arc::new(RwLock::new(None)),
        storage: Arc::new(RwLock::new(None)),
    };
    let storage_current = Arc::clone(&monitor.storage);
    let storage_pool = pool.clone();
    let storage_initialized = initialized.clone();
    tokio::spawn(async move {
        crate::database::wait_for_initialization(&storage_initialized).await;
        let mut interval = tokio::time::interval(STORAGE_SAMPLE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let binary_bytes = executable_bytes();
            let root = web_root.clone();
            let web_bytes = tokio::task::spawn_blocking(move || bounded_tree_bytes(&root))
                .await.ok().flatten();
            let database_bytes = sqlx::query_scalar::<_, i64>("SELECT pg_database_size(current_database())")
                .fetch_one(&storage_pool).await.ok().and_then(|value| u64::try_from(value).ok());
            let total_bytes = binary_bytes.zip(web_bytes)
                .and_then(|(binary, web)| binary.checked_add(web))
                .zip(database_bytes).and_then(|(subtotal, database)| subtotal.checked_add(database));
            let metric = PlatformStorageMetric {
                collected_at: Utc::now(), binary_bytes, state_bytes: None,
                web_bytes, database_bytes, total_bytes,
            };
            *storage_current.write().await = Some(metric.clone());
            if let Err(error) = sqlx::query(
                "INSERT INTO server_storage_samples (sampled_at, binary_bytes, web_bytes, database_bytes, total_bytes) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(metric.collected_at)
            .bind(metric.binary_bytes.and_then(|bytes| i64::try_from(bytes).ok()))
            .bind(metric.web_bytes.and_then(|bytes| i64::try_from(bytes).ok()))
            .bind(metric.database_bytes.and_then(|bytes| i64::try_from(bytes).ok()))
            .bind(metric.total_bytes.and_then(|bytes| i64::try_from(bytes).ok()))
            .execute(&storage_pool).await {
                tracing::warn!(%error, "platform storage sample could not be stored");
            }
        }
    });
    let current = Arc::clone(&monitor.current);
    tokio::spawn(async move {
        crate::database::wait_for_initialization(&initialized).await;
        let mut sampler = ProcessSampler::default();
        let mut policy_changes = settings.sender.subscribe();
        let mut seconds = settings.current().server_process_seconds;
        let mut next_sample = tokio::time::Instant::now();
        let mut last_warning: Option<Instant> = None;
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(next_sample) => {},
                result = policy_changes.changed() => {
                    if result.is_err() { break; }
                    let next_seconds = settings.current().server_process_seconds;
                    if next_seconds != seconds {
                        seconds = next_seconds;
                        next_sample = tokio::time::Instant::now() + Duration::from_secs(seconds);
                    }
                    continue;
                }
            }
            next_sample = tokio::time::Instant::now() + Duration::from_secs(seconds);
            let sampled_at = Utc::now();
            let metric = sampler.sample();
            *current.write().await = Some(ProcessSnapshot {
                sampled_at,
                metric: metric.clone(),
            });
            let result = sqlx::query(
                "INSERT INTO platform_process_samples (sampled_at, pid, cpu_percent_of_host, \
                 resident_memory_bytes, disk_read_bytes_per_second, disk_written_bytes_per_second) \
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(sampled_at)
            .bind(i64::from(metric.pid))
            .bind(metric.cpu_percent_of_host)
            .bind(
                metric
                    .resident_memory_bytes
                    .and_then(|bytes| i64::try_from(bytes).ok()),
            )
            .bind(metric.disk_read_bytes_per_second)
            .bind(metric.disk_written_bytes_per_second)
            .execute(&pool)
            .await;
            if let Err(error) = result {
                if last_warning.is_none_or(|at| at.elapsed() >= Duration::from_secs(300)) {
                    tracing::warn!(%error, "platform process sample could not be stored");
                    last_warning = Some(Instant::now());
                }
            }
        }
    });
    monitor
}

#[derive(Clone)]
struct PerformanceState {
    pool: PgPool,
    hostname: String,
    monitor: PerformanceMonitor,
    cache: Arc<Mutex<Option<(Instant, Value)>>>,
}

pub fn router(pool: PgPool, monitor: PerformanceMonitor) -> Router {
    Router::new()
        .route("/api/v1/platform-performance", get(performance_24h))
        .with_state(PerformanceState {
            pool,
            monitor,
            hostname: std::fs::read_to_string("/proc/sys/kernel/hostname")
                .map(|value| value.trim().to_owned())
                .unwrap_or_else(|_| "平台主机".to_owned()),
            cache: Arc::new(Mutex::new(None)),
        })
}

#[derive(Serialize)]
struct MetricStats {
    average: Option<f64>,
    maximum: Option<f64>,
    samples: i64,
}

#[derive(Serialize)]
struct ProcessWindow {
    sample_count: i64,
    first_sample_at: Option<DateTime<Utc>>,
    last_sample_at: Option<DateTime<Utc>>,
    cpu_percent_of_host: MetricStats,
    resident_memory_bytes: MetricStats,
    disk_read_bytes_per_second: MetricStats,
    disk_written_bytes_per_second: MetricStats,
}

#[derive(Serialize)]
struct StorageWindow {
    total_bytes: MetricStats,
    last_sample_at: Option<DateTime<Utc>>,
    latest: Option<PlatformStorageMetric>,
}

fn storage_window(row: &PgRow, latest: Option<PlatformStorageMetric>) -> Result<StorageWindow, sqlx::Error> {
    Ok(StorageWindow {
        total_bytes: metric_stats(row, "storage")?,
        last_sample_at: row.try_get("storage_last_sample_at")?,
        latest,
    })
}

fn metric_stats(row: &PgRow, name: &str) -> Result<MetricStats, sqlx::Error> {
    let average = format!("{name}_average");
    let maximum = format!("{name}_maximum");
    let samples = format!("{name}_samples");
    Ok(MetricStats {
        average: row.try_get(average.as_str())?,
        maximum: row.try_get(maximum.as_str())?,
        samples: row.try_get(samples.as_str())?,
    })
}

fn process_window(row: &PgRow) -> Result<ProcessWindow, sqlx::Error> {
    Ok(ProcessWindow {
        sample_count: row.try_get("sample_count")?,
        first_sample_at: row.try_get("first_sample_at")?,
        last_sample_at: row.try_get("last_sample_at")?,
        cpu_percent_of_host: metric_stats(row, "cpu")?,
        resident_memory_bytes: metric_stats(row, "memory")?,
        disk_read_bytes_per_second: metric_stats(row, "disk_read")?,
        disk_written_bytes_per_second: metric_stats(row, "disk_write")?,
    })
}

const SERVER_SUMMARY: &str = r"
    SELECT COUNT(*) AS sample_count,
           MIN(sampled_at) AS first_sample_at, MAX(sampled_at) AS last_sample_at,
           COUNT(cpu_percent_of_host) AS cpu_samples,
           AVG(cpu_percent_of_host) AS cpu_average,
           MAX(cpu_percent_of_host) AS cpu_maximum,
           COUNT(resident_memory_bytes) AS memory_samples,
           AVG(resident_memory_bytes::DOUBLE PRECISION) AS memory_average,
           MAX(resident_memory_bytes::DOUBLE PRECISION) AS memory_maximum,
           COUNT(disk_read_bytes_per_second) AS disk_read_samples,
           AVG(disk_read_bytes_per_second) AS disk_read_average,
           MAX(disk_read_bytes_per_second) AS disk_read_maximum,
           COUNT(disk_written_bytes_per_second) AS disk_write_samples,
           AVG(disk_written_bytes_per_second) AS disk_write_average,
           MAX(disk_written_bytes_per_second) AS disk_write_maximum
    FROM platform_process_samples WHERE sampled_at >= $1 AND sampled_at <= $2
";

const AGENT_SUMMARY: &str = r"
    SELECT n.id, n.hostname, n.last_seen,
           COUNT(s.agent_pid) AS sample_count,
           MIN(s.collected_at) FILTER (WHERE s.agent_pid IS NOT NULL) AS first_sample_at,
           MAX(s.collected_at) FILTER (WHERE s.agent_pid IS NOT NULL) AS last_sample_at,
           COUNT(s.agent_cpu_percent_of_host) AS cpu_samples,
           AVG(s.agent_cpu_percent_of_host) AS cpu_average,
           MAX(s.agent_cpu_percent_of_host) AS cpu_maximum,
           COUNT(s.agent_resident_memory_bytes) AS memory_samples,
           AVG(s.agent_resident_memory_bytes::DOUBLE PRECISION) AS memory_average,
           MAX(s.agent_resident_memory_bytes::DOUBLE PRECISION) AS memory_maximum,
           COUNT(s.agent_disk_read_bytes_per_second) AS disk_read_samples,
           AVG(s.agent_disk_read_bytes_per_second) AS disk_read_average,
           MAX(s.agent_disk_read_bytes_per_second) AS disk_read_maximum,
           COUNT(s.agent_disk_written_bytes_per_second) AS disk_write_samples,
           AVG(s.agent_disk_written_bytes_per_second) AS disk_write_average,
           MAX(s.agent_disk_written_bytes_per_second) AS disk_write_maximum
    FROM nodes n LEFT JOIN metrics_samples s
      ON s.node_id = n.id AND s.collected_at >= $1 AND s.collected_at <= $2
    WHERE n.deleted_at IS NULL
    GROUP BY n.id, n.hostname, n.last_seen ORDER BY n.hostname
";

const SERVER_STORAGE_SUMMARY: &str = r"
    SELECT COUNT(total_bytes) AS storage_samples,
           AVG(total_bytes::DOUBLE PRECISION) AS storage_average,
           MAX(total_bytes::DOUBLE PRECISION) AS storage_maximum,
           MAX(sampled_at) AS storage_last_sample_at
    FROM server_storage_samples WHERE sampled_at >= $1 AND sampled_at <= $2
";

const AGENT_STORAGE_SUMMARY: &str = r"
    SELECT n.id,
           (SELECT m.metrics->'platform_storage' FROM metrics_latest m WHERE m.node_id=n.id) AS latest_storage,
           storage.storage_samples, storage.storage_average,
           storage.storage_maximum, storage.storage_last_sample_at
    FROM nodes n
    LEFT JOIN LATERAL (
        SELECT COUNT(s.total_bytes) AS storage_samples,
               AVG(s.total_bytes::DOUBLE PRECISION) AS storage_average,
               MAX(s.total_bytes::DOUBLE PRECISION) AS storage_maximum,
               MAX(s.sampled_at) AS storage_last_sample_at
        FROM agent_storage_samples s
        WHERE s.node_id=n.id AND s.sampled_at >= $1 AND s.sampled_at <= $2
    ) storage ON TRUE
    WHERE n.deleted_at IS NULL
    ORDER BY n.hostname
";

async fn performance_24h(
    State(state): State<PerformanceState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut cache = state.cache.lock().await;
    if let Some((updated, value)) = cache.as_ref()
        && updated.elapsed() < SUMMARY_CACHE
    {
        return Ok(Json(value.clone()));
    }
    let generated_at = Utc::now();
    let window_start = generated_at - chrono::Duration::hours(24);
    let rows = tokio::try_join!(
        sqlx::query(SERVER_SUMMARY)
            .bind(window_start)
            .bind(generated_at)
            .fetch_one(&state.pool),
        sqlx::query(AGENT_SUMMARY)
            .bind(window_start)
            .bind(generated_at)
            .fetch_all(&state.pool),
        sqlx::query(SERVER_STORAGE_SUMMARY)
            .bind(window_start)
            .bind(generated_at)
            .fetch_one(&state.pool),
        sqlx::query(AGENT_STORAGE_SUMMARY)
            .bind(window_start)
            .bind(generated_at)
            .fetch_all(&state.pool),
    );
    let (server_row, agent_rows, server_storage_row, agent_storage_rows) = rows.map_err(|error| {
        tracing::warn!(%error, "platform performance history query failed");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "message": "24 小时进程历史暂不可读取" })),
        )
    })?;
    let server = process_window(&server_row).map_err(|error| {
        tracing::warn!(%error, "invalid controller process summary");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "message": "平台进程统计数据无效" })),
        )
    })?;
    let server_storage = storage_window(&server_storage_row, state.monitor.current_storage().await)
        .map_err(summary_error)?;
    let mut storage_by_node: HashMap<Uuid, PgRow> = HashMap::with_capacity(agent_storage_rows.len());
    for row in agent_storage_rows {
        let id: Uuid = row.try_get("id").map_err(summary_error)?;
        storage_by_node.insert(id, row);
    }
    let mut agents = Vec::with_capacity(agent_rows.len());
    for row in agent_rows {
        let id: Uuid = row.try_get("id").map_err(summary_error)?;
        let hostname: String = row.try_get("hostname").map_err(summary_error)?;
        let last_seen: DateTime<Utc> = row.try_get("last_seen").map_err(summary_error)?;
        let stats = process_window(&row).map_err(summary_error)?;
        let storage = storage_by_node.remove(&id)
            .map(|storage_row| {
                let latest = storage_row.try_get::<Option<Value>, _>("latest_storage")?
                    .and_then(|value| serde_json::from_value(value).ok());
                storage_window(&storage_row, latest)
            })
            .transpose().map_err(summary_error)?;
        agents.push(json!({
            "node_id": id, "hostname": hostname,
            "online": (generated_at - last_seen).num_seconds() <= 45,
            "last_seen": last_seen, "stats": stats, "storage": storage,
        }));
    }
    let value = json!({
        "generated_at": generated_at,
        "window_start": window_start,
        "window_end": generated_at,
        "period_seconds": 86_400,
        "server": { "hostname": state.hostname, "stats": server, "storage": server_storage },
        "agents": agents,
    });
    *cache = Some((Instant::now(), value.clone()));
    Ok(Json(value))
}

fn summary_error(error: sqlx::Error) -> (StatusCode, Json<Value>) {
    tracing::warn!(%error, "invalid Agent process summary");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "message": "Agent 进程统计数据无效" })),
    )
}
