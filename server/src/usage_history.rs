//! On-demand, seven-calendar-day history. No Agent work or periodic query.
//! A separate lazy read-only pool, single query permit, bounded stream and
//! expiring aggregate cache isolate this workload from the ingestion pool.
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Duration as ChronoDuration, FixedOffset, NaiveDate, TimeZone, Utc};
use easymanage_protocol::{GpuMetric, HostTemperatures};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct};
use serde_json::{Value, json};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use uuid::Uuid;

const BUCKET_SECONDS: i64 = 600;
const CACHE_TTL: Duration = Duration::from_secs(300);
const MAX_CACHE_ENTRIES: usize = 8;
const MAX_ROWS: usize = 250_000;
const MAX_GPUS: usize = 64;

#[derive(Clone)]
struct HistoryState {
    pool: PgPool,
    gate: Arc<Semaphore>,
    cache: Arc<Mutex<HashMap<CacheKey, Cached>>>,
}
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct CacheKey {
    node: Uuid,
    date: NaiveDate,
    offset: i32,
}
struct Cached {
    at: Instant,
    value: Bytes,
}

pub fn router(pool: &PgPool) -> Router {
    let reader = PgPoolOptions::new()
        .max_connections(1)
        .min_connections(0)
        .acquire_timeout(Duration::from_secs(2))
        .idle_timeout(Some(Duration::from_secs(120)))
        .after_connect(|connection, _| {
            Box::pin(async move {
                for command in [
                    "SET default_transaction_read_only=on",
                    "SET statement_timeout='5s'",
                    "SET lock_timeout='250ms'",
                    "SET work_mem='4MB'",
                    "SET jit=off",
                    "SET max_parallel_workers_per_gather=0",
                ] {
                    sqlx::query(command).execute(&mut *connection).await?;
                }
                Ok(())
            })
        })
        .connect_lazy_with(
            pool.connect_options()
                .as_ref()
                .clone()
                .application_name("easymanage-usage-history"),
        );
    Router::new()
        .route("/api/v1/nodes/{node_id}/usage-history", get(history))
        .with_state(HistoryState {
            pool: reader,
            gate: Arc::new(Semaphore::new(1)),
            cache: Arc::default(),
        })
        .layer(tower_http::compression::CompressionLayer::new())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryQuery {
    #[serde(default = "default_offset")]
    utc_offset_minutes: i32,
}
fn default_offset() -> i32 {
    480
}

#[derive(Debug)]
struct HistoryError(StatusCode, &'static str);
impl IntoResponse for HistoryError {
    fn into_response(self) -> Response {
        let mut result = (self.0, Json(json!({"message":self.1}))).into_response();
        result
            .headers_mut()
            .insert("cache-control", HeaderValue::from_static("no-store"));
        if self.0 == StatusCode::TOO_MANY_REQUESTS {
            result
                .headers_mut()
                .insert("retry-after", HeaderValue::from_static("5"));
        }
        result
    }
}
fn unavailable(_: sqlx::Error) -> HistoryError {
    // Do not expose connection strings, SQL or database diagnostics.
    HistoryError(
        StatusCode::SERVICE_UNAVAILABLE,
        "历史数据暂不可用，请稍后手动重试",
    )
}

struct Window {
    date: NaiveDate,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    offset: i32,
}
impl Window {
    fn new(now: DateTime<Utc>, offset: i32) -> Result<Self, HistoryError> {
        if !(-840..=840).contains(&offset) || offset % 15 != 0 {
            return Err(HistoryError(StatusCode::BAD_REQUEST, "时区偏移无效"));
        }
        let zone = FixedOffset::east_opt(offset * 60)
            .ok_or(HistoryError(StatusCode::BAD_REQUEST, "时区偏移无效"))?;
        let date = now.with_timezone(&zone).date_naive();
        let first = date - ChronoDuration::days(6);
        let start = zone
            .from_local_datetime(&first.and_hms_opt(0, 0, 0).expect("midnight"))
            .single()
            .expect("fixed offset")
            .with_timezone(&Utc);
        Ok(Self {
            date,
            start,
            end: now,
            offset,
        })
    }
    fn day(&self, at: DateTime<Utc>) -> Option<usize> {
        if at < self.start || at >= self.end {
            return None;
        }
        Some(((at - self.start).num_seconds() / 86400) as usize)
    }
}

async fn history(
    State(state): State<HistoryState>,
    Path(node): Path<Uuid>,
    Query(query): Query<HistoryQuery>,
) -> Result<Response, HistoryError> {
    let window = Window::new(Utc::now(), query.utc_offset_minutes)?;
    let key = CacheKey {
        node,
        date: window.date,
        offset: window.offset,
    };
    let cached = state.cache.lock().ok().and_then(|cache| {
        cache
            .get(&key)
            .filter(|item| item.at.elapsed() < CACHE_TTL)
            .map(|item| item.value.clone())
    });
    if let Some(value) = cached {
        return Ok(json_response(value, "hit"));
    }
    let _permit = state.gate.clone().try_acquire_owned().map_err(|_| {
        HistoryError(
            StatusCode::TOO_MANY_REQUESTS,
            "已有历史查询正在执行，请稍后手动重试",
        )
    })?;
    let value = tokio::time::timeout(
        Duration::from_secs(10),
        read_history(&state.pool, node, &window),
    )
    .await
    .map_err(|_| {
        HistoryError(
            StatusCode::GATEWAY_TIMEOUT,
            "历史查询超过时间预算，请稍后手动重试",
        )
    })??;
    let value = Bytes::from(
        serde_json::to_vec(&value)
            .map_err(|_| HistoryError(StatusCode::INTERNAL_SERVER_ERROR, "历史汇总生成失败"))?,
    );
    if let Ok(mut cache) = state.cache.lock() {
        cache.retain(|_, item| item.at.elapsed() < CACHE_TTL);
        if cache.len() >= MAX_CACHE_ENTRIES {
            let oldest = cache
                .iter()
                .min_by_key(|(_, item)| item.at)
                .map(|(key, _)| *key);
            if let Some(key) = oldest {
                cache.remove(&key);
            }
        }
        cache.insert(
            key,
            Cached {
                at: Instant::now(),
                value: value.clone(),
            },
        );
    }
    Ok(json_response(value, "miss"))
}
fn json_response(value: Bytes, cache: &'static str) -> Response {
    // Cache the serialized payload once; repeated viewers only clone Bytes.
    let mut response = value.into_response();
    response
        .headers_mut()
        .insert("content-type", HeaderValue::from_static("application/json"));
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert("x-history-cache", HeaderValue::from_static(cache));
    response
}

#[derive(Clone, Default)]
struct Stat {
    sum: f64,
    maximum: Option<f64>,
    minimum: Option<f64>,
    samples: u64,
}
impl Stat {
    fn add(&mut self, value: f64) {
        if !value.is_finite() || value < 0.0 {
            return;
        }
        self.sum += value;
        self.samples += 1;
        self.maximum = Some(self.maximum.map_or(value, |old| old.max(value)));
        self.minimum = Some(self.minimum.map_or(value, |old| old.min(value)));
    }
}
impl Serialize for Stat {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Stat", 4)?;
        state.serialize_field(
            "average",
            &if self.samples > 0 {
                Some(self.sum / self.samples as f64)
            } else {
                None
            },
        )?;
        state.serialize_field("maximum", &self.maximum)?;
        state.serialize_field("minimum", &self.minimum)?;
        state.serialize_field("samples", &self.samples)?;
        state.end()
    }
}
#[derive(Clone, Default, Serialize)]
struct ThrottleStat {
    samples: u64,
    detected_samples: u64,
}
impl ThrottleStat {
    fn add(&mut self, value: Option<bool>) {
        if let Some(detected) = value {
            self.samples += 1;
            self.detected_samples += u64::from(detected);
        }
    }
}
#[derive(Clone, Default, Serialize)]
struct GpuStats {
    core: Stat,
    memory: Stat,
    memory_percent: Stat,
    temperature: Stat,
    thermal_throttling: ThrottleStat,
}
#[derive(Clone, Default, Serialize)]
struct Aggregate {
    cpu: Stat,
    memory: Stat,
    network_rx: Stat,
    network_tx: Stat,
    disk_read: Stat,
    disk_write: Stat,
    cpu_temperature: Stat,
    cpu_thermal_throttling: ThrottleStat,
    ssd_temperature: Stat,
    gpu_count: Stat,
    gpu: GpuStats,
    gpus: BTreeMap<u32, GpuStats>,
}
enum Event<'a> {
    Fast {
        cpu: f64,
        memory: f64,
        rx: f64,
        tx: f64,
    },
    Disk {
        read: Option<f64>,
        written: Option<f64>,
    },
    Thermal {
        cpu: Option<f32>,
        ssd: Option<f32>,
        cpu_throttling: Option<bool>,
    },
    Gpu(&'a [GpuMetric]),
}
fn valid_temperature(value: f32) -> bool {
    value.is_finite() && (0.0..=150.0).contains(&value)
}
impl GpuStats {
    fn add(&mut self, gpu: &GpuMetric) {
        self.thermal_throttling.add(gpu.thermal_throttling);
        if gpu.utilization_percent.is_finite() && (0.0..=100.0).contains(&gpu.utilization_percent) {
            self.core.add(f64::from(gpu.utilization_percent));
        }
        if gpu.memory_total_bytes > 0 && gpu.memory_used_bytes <= gpu.memory_total_bytes {
            self.memory.add(gpu.memory_used_bytes as f64);
            self.memory_percent
                .add(gpu.memory_used_bytes as f64 / gpu.memory_total_bytes as f64 * 100.0);
        }
        if let Some(value) = gpu.temperature_celsius.filter(|v| valid_temperature(*v)) {
            self.temperature.add(f64::from(value));
        }
    }
}
impl Aggregate {
    fn add(&mut self, event: &Event<'_>) {
        match event {
            Event::Fast {
                cpu,
                memory,
                rx,
                tx,
            } => {
                self.cpu.add(*cpu);
                self.memory.add(*memory);
                self.network_rx.add(*rx);
                self.network_tx.add(*tx);
            }
            Event::Disk { read, written } => {
                if let Some(v) = read {
                    self.disk_read.add(*v);
                }
                if let Some(v) = written {
                    self.disk_write.add(*v);
                }
            }
            Event::Thermal {
                cpu,
                ssd,
                cpu_throttling,
            } => {
                self.cpu_thermal_throttling.add(*cpu_throttling);
                if let Some(v) = cpu.filter(|v| valid_temperature(*v)) {
                    self.cpu_temperature.add(f64::from(v));
                }
                if let Some(v) = ssd.filter(|v| valid_temperature(*v)) {
                    self.ssd_temperature.add(f64::from(v));
                }
            }
            Event::Gpu(gpus) => {
                if gpus.is_empty() {
                    return;
                }
                self.gpu_count.add(gpus.len() as f64);
                let mut snapshot = GpuStats::default();
                let mut total = 0_u64;
                let mut used = 0_u64;
                for gpu in *gpus {
                    self.gpus.entry(gpu.index).or_default().add(gpu);
                    snapshot.add(gpu);
                    if gpu.memory_total_bytes > 0 && gpu.memory_used_bytes <= gpu.memory_total_bytes
                    {
                        total = total.saturating_add(gpu.memory_total_bytes);
                        used = used.saturating_add(gpu.memory_used_bytes);
                    }
                }
                if snapshot.core.samples > 0 {
                    self.gpu
                        .core
                        .add(snapshot.core.sum / snapshot.core.samples as f64);
                }
                if total > 0 {
                    self.gpu.memory.add(used as f64);
                    self.gpu
                        .memory_percent
                        .add(used as f64 / total as f64 * 100.0);
                }
                if let Some(v) = snapshot.temperature.maximum {
                    self.gpu.temperature.add(v);
                }
                self.gpu.thermal_throttling.add(
                    if snapshot.thermal_throttling.detected_samples > 0 {
                        Some(true)
                    } else if snapshot.thermal_throttling.samples == gpus.len() as u64 {
                        Some(false)
                    } else {
                        None
                    },
                );
            }
        }
    }
}
struct Accumulator {
    total: Aggregate,
    days: [Aggregate; 7],
    buckets: BTreeMap<i64, Aggregate>,
    fast: HashSet<DateTime<Utc>>,
    disk: HashSet<DateTime<Utc>>,
    thermal: HashSet<DateTime<Utc>>,
    gpu: HashSet<DateTime<Utc>>,
    catalog: BTreeMap<u32, (DateTime<Utc>, String)>,
}
impl Default for Accumulator {
    fn default() -> Self {
        Self {
            total: Aggregate::default(),
            days: std::array::from_fn(|_| Aggregate::default()),
            buckets: BTreeMap::new(),
            fast: HashSet::new(),
            disk: HashSet::new(),
            thermal: HashSet::new(),
            gpu: HashSet::new(),
            catalog: BTreeMap::new(),
        }
    }
}
impl Accumulator {
    fn add(&mut self, window: &Window, at: DateTime<Utc>, event: Event<'_>) {
        let Some(day) = window.day(at) else {
            return;
        };
        let seen = match &event {
            Event::Fast { .. } => &mut self.fast,
            Event::Disk { .. } => &mut self.disk,
            Event::Thermal { .. } => &mut self.thermal,
            Event::Gpu(_) => &mut self.gpu,
        };
        if !seen.insert(at) {
            return;
        }
        if let Event::Gpu(gpus) = &event {
            for gpu in *gpus {
                if self
                    .catalog
                    .get(&gpu.index)
                    .is_none_or(|(last, _)| at >= *last)
                {
                    self.catalog.insert(gpu.index, (at, gpu.name.clone()));
                }
            }
        }
        let bucket = (at - window.start).num_seconds() / BUCKET_SECONDS;
        self.total.add(&event);
        self.days[day].add(&event);
        self.buckets.entry(bucket).or_default().add(&event);
    }
}

async fn read_history(pool: &PgPool, node: Uuid, window: &Window) -> Result<Value, HistoryError> {
    let name:String=sqlx::query_scalar("SELECT COALESCE(NULLIF(display_name,''),hostname) FROM nodes WHERE id=$1 AND deleted_at IS NULL")
        .bind(node).fetch_optional(pool).await.map_err(unavailable)?
        .ok_or(HistoryError(StatusCode::NOT_FOUND,"机器不存在或已删除"))?;
    // Aggregate while streaming; sorting raw wide GPU rows in PostgreSQL would
    // spill to disk. Bucket order and latest GPU labels are maintained here.
    let mut rows=sqlx::query("SELECT collected_at,fast_collected_at,cpu_usage_percent,memory_used_bytes,network_received_bytes_per_second,network_transmitted_bytes_per_second,disk_collected_at,disk_read_bytes_per_second,disk_written_bytes_per_second,gpu_collected_at,gpu_metrics,temperatures FROM metrics_samples WHERE node_id=$1 AND collected_at >= $2 AND collected_at < $3 LIMIT $4")
        .bind(node).bind(window.start).bind(window.end).bind((MAX_ROWS+1) as i64).fetch(pool);
    let mut accumulator = Accumulator::default();
    let mut count = 0;
    while let Some(row) = rows.try_next().await.map_err(unavailable)? {
        count += 1;
        if count > MAX_ROWS {
            return Err(HistoryError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "历史样本超过单次读取预算，请联系管理员检查采样数据",
            ));
        }
        let at: DateTime<Utc> = row.try_get("collected_at").map_err(unavailable)?;
        let fast = row
            .try_get::<Option<DateTime<Utc>>, _>("fast_collected_at")
            .map_err(unavailable)?
            .unwrap_or(at);
        accumulator.add(
            window,
            fast,
            Event::Fast {
                cpu: row.try_get("cpu_usage_percent").map_err(unavailable)?,
                memory: row
                    .try_get::<i64, _>("memory_used_bytes")
                    .map_err(unavailable)? as f64,
                rx: row
                    .try_get("network_received_bytes_per_second")
                    .map_err(unavailable)?,
                tx: row
                    .try_get("network_transmitted_bytes_per_second")
                    .map_err(unavailable)?,
            },
        );
        if let Some(at) = row
            .try_get::<Option<DateTime<Utc>>, _>("disk_collected_at")
            .map_err(unavailable)?
        {
            accumulator.add(
                window,
                at,
                Event::Disk {
                    read: row
                        .try_get("disk_read_bytes_per_second")
                        .map_err(unavailable)?,
                    written: row
                        .try_get("disk_written_bytes_per_second")
                        .map_err(unavailable)?,
                },
            );
        }
        let gpus: Vec<GpuMetric> = serde_json::from_value(
            row.try_get("gpu_metrics").map_err(unavailable)?,
        )
        .map_err(|_| HistoryError(StatusCode::SERVICE_UNAVAILABLE, "历史 GPU 数据格式不可读取"))?;
        if gpus.len() > MAX_GPUS {
            return Err(HistoryError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "GPU 数量超过当前历史查询预算",
            ));
        }
        if !gpus.is_empty() {
            let at = row
                .try_get::<Option<DateTime<Utc>>, _>("gpu_collected_at")
                .map_err(unavailable)?
                .unwrap_or(at);
            accumulator.add(window, at, Event::Gpu(&gpus));
            if accumulator.catalog.len() > MAX_GPUS {
                return Err(HistoryError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "历史 GPU 数量超过当前查询预算",
                ));
            }
        }
        if let Some(value) = row
            .try_get::<Option<Value>, _>("temperatures")
            .map_err(unavailable)?
        {
            let temperatures: HostTemperatures = serde_json::from_value(value).map_err(|_| {
                HistoryError(StatusCode::SERVICE_UNAVAILABLE, "历史温度数据格式不可读取")
            })?;
            let hottest = temperatures
                .ssds
                .iter()
                .map(|ssd| ssd.temperature_celsius)
                .filter(|v| valid_temperature(*v))
                .max_by(f32::total_cmp);
            accumulator.add(
                window,
                temperatures.collected_at,
                Event::Thermal {
                    cpu: temperatures.cpu_celsius,
                    ssd: hottest,
                    cpu_throttling: temperatures.cpu_thermal_throttling,
                },
            );
        }
        if count % 256 == 0 {
            tokio::task::yield_now().await;
        }
    }
    let days:Vec<Value>=accumulator.days.iter().enumerate().map(|(i,summary)| {
        let start=window.start+ChronoDuration::days(i as i64);let end=(start+ChronoDuration::days(1)).min(window.end);
        json!({"date":(window.date-ChronoDuration::days(6-i as i64)).to_string(),"start":start,"end":end,"summary":summary})
    }).collect();
    let points:Vec<Value>=accumulator.buckets.iter().map(|(bucket,summary)|json!({"at":window.start+ChronoDuration::seconds(bucket*BUCKET_SECONDS),"stats":summary})).collect();
    let gpus: Vec<Value> = accumulator
        .catalog
        .iter()
        .map(|(index, (at, name))| json!({"index":index,"name":name,"last_seen":at}))
        .collect();
    Ok(
        json!({"node_id":node,"hostname":name,"generated_at":Utc::now(),"start":window.start,"end":window.end,"utc_offset_minutes":window.offset,
        "bucket_seconds":BUCKET_SECONDS,"cache_seconds":CACHE_TTL.as_secs(),"source_rows":count,"summary":accumulator.total,"days":days,"gpus":gpus,"points":points}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }
    #[test]
    fn calendar_window_uses_explicit_offset_and_contains_today() {
        let window = Window::new(at("2026-11-01T10:00:00Z"), 480).unwrap();
        assert_eq!(window.start, at("2026-10-25T16:00:00Z"));
        assert_eq!(window.date.to_string(), "2026-11-01");
        assert_eq!(window.day(at("2026-10-31T16:00:00Z")), Some(6));
        assert_eq!(window.day(window.end), None);
        assert!(Window::new(window.end, 841).is_err());
    }
    #[test]
    fn repeated_cached_timestamps_and_missing_data_are_not_extra_samples() {
        let window = Window::new(at("2026-11-01T10:00:00Z"), 480).unwrap();
        let mut a = Accumulator::default();
        let t = window.start + ChronoDuration::seconds(1);
        for _ in 0..3 {
            a.add(
                &window,
                t,
                Event::Disk {
                    read: Some(20.0),
                    written: None,
                },
            );
        }
        assert_eq!(a.total.disk_read.samples, 1);
        assert_eq!(a.total.disk_write.samples, 0);
        assert_eq!(
            serde_json::to_value(&a.total.disk_write).unwrap()["average"],
            Value::Null
        );
        assert_eq!(a.days[1].disk_read.samples, 0);
        assert_eq!(a.buckets.len(), 1);
    }
    #[test]
    fn historical_gpus_survive_disappearance_and_keep_independent_peaks() {
        let window = Window::new(at("2026-11-01T10:00:00Z"), 480).unwrap();
        let mut a = Accumulator::default();
        let gpu = |index, core| GpuMetric {
            index,
            name: format!("GPU {index}"),
            utilization_percent: core,
            memory_used_bytes: 20,
            memory_total_bytes: 100,
            temperature_celsius: Some(75.0),
            power_watts: None,
            thermal_throttling: None,
        };
        a.add(
            &window,
            window.start + ChronoDuration::seconds(1),
            Event::Gpu(&[gpu(0, 10.0), gpu(7, 80.0)]),
        );
        a.add(
            &window,
            window.start + ChronoDuration::seconds(601),
            Event::Gpu(&[gpu(0, 30.0)]),
        );
        assert_eq!(a.catalog.len(), 2);
        assert_eq!(a.total.gpus[&7].core.samples, 1);
        assert!(!a.buckets[&1].gpus.contains_key(&7));
        assert_eq!(a.total.gpu_count.minimum, Some(1.0));
        assert_eq!(a.total.gpu.core.maximum, Some(45.0));
    }
    #[test]
    fn unsorted_rows_keep_latest_gpu_labels_and_identical_summary_values() {
        let window = Window::new(at("2026-11-01T10:00:00Z"), 480).unwrap();
        let mut a = Accumulator::default();
        let mut gpu = GpuMetric {
            index: 0,
            name: "Latest model".into(),
            utilization_percent: 20.0,
            memory_used_bytes: 10,
            memory_total_bytes: 100,
            temperature_celsius: None,
            power_watts: None,
            thermal_throttling: None,
        };
        a.add(
            &window,
            window.start + ChronoDuration::hours(2),
            Event::Gpu(&[gpu.clone()]),
        );
        gpu.name = "Earlier model".into();
        gpu.utilization_percent = 80.0;
        a.add(
            &window,
            window.start + ChronoDuration::hours(1),
            Event::Gpu(&[gpu]),
        );
        assert_eq!(a.catalog[&0].1, "Latest model");
        assert_eq!(a.total.gpu.core.sum, 100.0);
        assert_eq!(a.total.gpu.core.samples, 2);
    }

    #[test]
    fn overheating_requires_recorded_thermal_throttle_evidence_and_deduplicates_samples() {
        let window = Window::new(at("2026-11-01T10:00:00Z"), 480).unwrap();
        let mut a = Accumulator::default();
        let first = window.start + ChronoDuration::seconds(1);
        a.add(
            &window,
            first,
            Event::Thermal {
                cpu: Some(100.0),
                ssd: Some(95.0),
                cpu_throttling: None,
            },
        );
        assert_eq!(a.total.cpu_thermal_throttling.samples, 0);
        let second = first + ChronoDuration::seconds(30);
        for _ in 0..3 {
            a.add(
                &window,
                second,
                Event::Thermal {
                    cpu: Some(60.0),
                    ssd: None,
                    cpu_throttling: Some(true),
                },
            );
        }
        assert_eq!(a.total.cpu_thermal_throttling.samples, 1);
        assert_eq!(a.total.cpu_thermal_throttling.detected_samples, 1);
        let mut gpu = GpuMetric {
            index: 7,
            name: "GPU 7".into(),
            utilization_percent: 80.0,
            memory_used_bytes: 10,
            memory_total_bytes: 100,
            temperature_celsius: Some(99.0),
            power_watts: None,
            thermal_throttling: Some(false),
        };
        a.add(&window, first, Event::Gpu(&[gpu.clone()]));
        assert_eq!(a.total.gpu.thermal_throttling.detected_samples, 0);
        gpu.temperature_celsius = Some(60.0);
        gpu.thermal_throttling = Some(true);
        for _ in 0..3 {
            a.add(&window, second, Event::Gpu(&[gpu.clone()]));
        }
        assert_eq!(a.total.gpus[&7].thermal_throttling.detected_samples, 1);
        assert_eq!(a.total.gpu.thermal_throttling.samples, 2);
        assert_eq!(a.total.gpu.thermal_throttling.detected_samples, 1);
        assert_eq!(a.days[1].gpu.thermal_throttling.samples, 0);
    }
}
