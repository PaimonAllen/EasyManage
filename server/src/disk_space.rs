//! On-demand disk metadata scans over the existing trusted SSH transport.
//! No Agent replacement, arbitrary commands, content reads or deletion endpoints.
use super::*;
use axum::extract::Query;
use axum::http::HeaderValue;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::process::Stdio;

#[path = "disk_space/live.rs"]
mod live;
#[path = "disk_space/safety.rs"]
mod safety;

const SCRIPT: &str = include_str!("../../deploy/linux/operations/scan-disk-space.py");
// A message/page bound, not a total scan or file-count limit.
const MAX_FRAME: usize = 1024 * 1024;
const MAX_SCANS: usize = 10;
const MAX_MEMORY_RESULTS: usize = 100;
const STORAGE_RESERVE: u64 = 2 * 1024 * 1024 * 1024;
const STORAGE_LOW: &str = "平台结果存储分区可用空间不足 2 GiB，已停止扫描以保护数据库；请先释放空间或迁移数据库，再重新扫描";
const DATABASE_FULL: &str = "数据库所在磁盘空间已耗尽，扫描结果写入失败；已写入条目保留，但缺少完整汇总。请先释放空间或迁移数据库，再重新扫描";

fn parse_available(bytes: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    let (blocks, size) = text.split_once(':')?;
    blocks
        .parse::<u64>()
        .ok()?
        .checked_mul(size.parse::<u64>().ok()?)
}

async fn storage_available() -> Result<u64, OperationsError> {
    // This is a local filesystem guard, not a claim about a remote PostgreSQL
    // server. Configure a visible backing mount when the database is elsewhere.
    let path = std::env::var_os("EASYMANAGE_DISK_SCAN_STORAGE_PATH").unwrap_or_else(|| "/".into());
    let mut command = tokio::process::Command::new("/usr/bin/stat");
    command
        .args(["-f", "--format=%a:%S", "--"])
        .arg(path)
        .env_clear()
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let value = tokio::time::timeout(Duration::from_secs(2), command.output())
        .await
        .ok()
        .and_then(Result::ok)
        .filter(|output| output.status.success())
        .and_then(|output| parse_available(&output.stdout));
    value.ok_or(OperationsError(
        StatusCode::SERVICE_UNAVAILABLE,
        "无法确认平台结果存储分区的可用空间，已停止扫描；请检查存储保护路径配置",
    ))
}

fn require_storage(available: u64) -> Result<(), OperationsError> {
    if available < STORAGE_RESERVE {
        Err(OperationsError(
            StatusCode::INSUFFICIENT_STORAGE,
            STORAGE_LOW,
        ))
    } else {
        Ok(())
    }
}

fn require_memory() -> Result<(), OperationsError> {
    let available = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                line.strip_prefix("MemAvailable:")
                    .and_then(|value| value.split_whitespace().next())
                    .and_then(|value| value.parse::<u64>().ok())
            })
        });
    if available.is_some_and(|kib| kib >= 512 * 1024) {
        Ok(())
    } else {
        Err(OperationsError(
            StatusCode::SERVICE_UNAVAILABLE,
            "平台可用内存不足 512 MiB 或无法确认；已停止实时扫描，内存中的已有结果仍可查看",
        ))
    }
}

fn db_query_error(error: sqlx::Error) -> OperationsError {
    let code = error.as_database_error().and_then(|error| error.code());
    // Record only SQLSTATE; SQL text, paths and credential context stay private.
    tracing::warn!(sqlstate = ?code, "disk scan database operation failed");
    if code.as_deref() == Some("53100") {
        OperationsError(StatusCode::INSUFFICIENT_STORAGE, DATABASE_FULL)
    } else {
        db_error()
    }
}

#[derive(Clone, Default)]
pub(super) struct Scans {
    active: Arc<Mutex<HashMap<Uuid, (Uuid, Arc<AtomicBool>)>>>,
    ready: Arc<AtomicBool>,
    live: Arc<Mutex<HashMap<Uuid, live::Job>>>,
    homes: Arc<Mutex<HashMap<Uuid, Arc<Mutex<HomeCache>>>>>,
    controls: Arc<Mutex<HashMap<Uuid, PauseControl>>>,
    reviews: Arc<Mutex<HashMap<Uuid, safety::Review>>>,
}

#[derive(Clone, Default)]
struct PauseControl {
    requested: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    clock: Arc<Mutex<(Option<Instant>, Duration)>>,
}
impl PauseControl {
    fn acknowledge(&self, paused: bool) {
        if let Ok(mut clock) = self.clock.lock() {
            if paused && clock.0.is_none() { clock.0 = Some(Instant::now()); }
            if !paused { if let Some(at) = clock.0.take() { clock.1 += at.elapsed(); } }
        }
        self.paused.store(paused, Ordering::Release);
    }
    fn elapsed(&self, since: Instant) -> Duration {
        let excluded = self.clock.lock().map(|clock| clock.1 + clock.0.map_or(Duration::ZERO, |at| at.elapsed())).unwrap_or(Duration::ZERO);
        since.elapsed().saturating_sub(excluded)
    }
}
fn scan_control(state: &OperationsState, id: Uuid, mut value: Value) -> Result<Value, OperationsError> {
    let controls = state.disk_space.controls.lock().map_err(|_| OperationsError::busy())?;
    let control = controls.get(&id);
    let paused = control.is_some_and(|c| c.paused.load(Ordering::Acquire));
    value["paused"] = json!(paused);
    value["pause_pending"] = json!(control.is_some_and(|c| c.requested.load(Ordering::Acquire) != paused));
    value["pause_supported"] = json!(control.is_some());
    Ok(value)
}

#[derive(Default)]
struct HomeCache { value: Option<(Instant, Vec<String>)> }

struct ScanLease {
    state: Scans,
    id: Uuid,
}
impl Drop for ScanLease {
    fn drop(&mut self) {
        if let Ok(mut active) = self.state.active.lock() { active.remove(&self.id); }
        if let Ok(mut controls) = self.state.controls.lock() { controls.remove(&self.id); }
    }
}

impl Scans {
    fn acquire(
        &self,
        id: Uuid,
        node: Uuid,
        request: &Request,
    ) -> Result<(ScanLease, Arc<AtomicBool>, Option<live::Job>), OperationsError> {
        if !self.ready.load(Ordering::Acquire) {
            return Err(OperationsError::busy());
        }
        let mut active = self.active.lock().map_err(|_| OperationsError::busy())?;
        if active.len() >= MAX_SCANS {
            return Err(OperationsError(
                StatusCode::CONFLICT,
                "平台同时扫描已达 10 个（包含暂停任务）；请等待或取消现有任务",
            ));
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.controls.lock().map_err(|_| OperationsError::busy())?.insert(id, PauseControl::default());
        active.insert(id, (node, stop.clone()));
        let lease = ScanLease { state: self.clone(), id };
        drop(active);
        // Admission and insertion share one cache lock: concurrent starts can
        // never exceed the result cap. No control-map lock is held here.
        let memory = if request.storage == "realtime" {
            self.reap_live()?;
            let mut jobs = self.live.lock().map_err(|_| OperationsError::busy())?;
            if jobs.len() >= MAX_MEMORY_RESULTS {
                let mut oldest = None;
                for (key, job) in jobs.iter() {
                    let job = job.lock().map_err(|_| OperationsError::busy())?;
                    if job.summary["status"] != "running" {
                        let at = job.summary["finished_at"].as_str().unwrap_or("").to_owned();
                        let candidate = (at, *key);
                        if oldest.as_ref().is_none_or(|old| &candidate < old) { oldest = Some(candidate); }
                    }
                }
                let Some((_, key)) = oldest else {
                    return Err(OperationsError(StatusCode::CONFLICT, "内存结果已达 100 份，且没有可清理的已结束结果"));
                };
                jobs.remove(&key);
            }
            let job = Arc::new(Mutex::new(live::LiveScan::new(node, id, request, stop.clone())));
            jobs.insert(id, job.clone());
            Some(job)
        } else { None };
        Ok((lease, stop, memory))
    }
}


struct ScanOperationLease { limits: Arc<Mutex<Limits>>, node: Uuid }
impl ScanOperationLease {
    fn acquire(shared: &Arc<Mutex<Limits>>, node: Uuid) -> Result<Self, OperationsError> {
        let mut limits = shared.lock().map_err(|_| OperationsError::busy())?;
        let now = Instant::now();
        limits.scan_attempts.retain(|(at, _)| now.duration_since(*at) < Duration::from_secs(60));
        if limits.elevating.contains(&node) || limits.scan_attempts.len() >= 120
            || limits.scan_attempts.iter().filter(|(_, id)| *id == node).count() >= 60 {
            return Err(OperationsError::busy());
        }
        limits.scan_attempts.push_back((now, node));
        *limits.disk_scans.entry(node).or_default() += 1;
        Ok(Self { limits: shared.clone(), node })
    }
}
impl Drop for ScanOperationLease {
    fn drop(&mut self) {
        if let Ok(mut limits) = self.limits.lock() {
            if let Some(count) = limits.disk_scans.get_mut(&self.node) {
                *count -= 1;
                if *count == 0 { limits.disk_scans.remove(&self.node); }
            }
        }
    }
}

pub(super) fn initialize(state: OperationsState) {
    let memory = state.disk_space.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        loop {
            tick.tick().await;
            let _ = memory.reap_live();
        }
    });
    tokio::spawn(async move {
        crate::database::wait_for_initialization(&state.initialized).await;
        // Never resume an expensive scan just because the controller restarted.
        loop {
            if sqlx::query("UPDATE disk_space_scans SET status='interrupted',finished_at=NOW(),message='平台已重启；扫描中断，请手动重新扫描' WHERE status='running'")
                .execute(&state.pool).await.is_ok() {
                state.disk_space.ready.store(true, Ordering::Release);
                break;
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        let mut tick = tokio::time::interval(Duration::from_hours(1));
        loop {
            tick.tick().await;
            let _ = prune(&state.pool, &state.disk_space).await;
        }
    });
}

async fn prune(pool: &PgPool, scans: &Scans) -> Result<(), OperationsError> {
    let active: Vec<Uuid> = scans
        .active
        .lock()
        .map_err(|_| OperationsError::busy())?
        .keys()
        .copied()
        .collect();
    sqlx::query("UPDATE disk_space_scans SET status='interrupted',finished_at=NOW(),message='扫描工作进程已结束但结果未保存，请重新扫描' WHERE status='running' AND NOT(id=ANY($1)) AND started_at<NOW()-INTERVAL '1 minute'")
        .bind(active).execute(pool).await.map_err(|_|db_error())?;
    sqlx::query("DELETE FROM disk_space_scans WHERE status<>'running' AND (started_at < NOW()-INTERVAL '7 days' OR id IN (SELECT id FROM (SELECT id,ROW_NUMBER() OVER(PARTITION BY node_id,mode ORDER BY started_at DESC) AS per_node,ROW_NUMBER() OVER(ORDER BY started_at DESC) AS total FROM disk_space_scans WHERE status<>'running') r WHERE per_node>3 OR total>30))")
        .execute(pool).await.map_err(|_| db_error())?;
    Ok(())
}

pub(super) fn router() -> Router<OperationsState> {
    Router::new()
        .route("/api/v1/nodes/{node_id}/disk-space", get(list).post(start))
        .route("/api/v1/nodes/{node_id}/disk-space/review", post(safety::review))
        .route(
            "/api/v1/nodes/{node_id}/disk-space/presence",
            post(live::presence),
        )
        .route("/api/v1/nodes/{node_id}/disk-space/homes", get(homes))
        .route("/api/v1/nodes/{node_id}/disk-space/{scan_id}", get(detail))
        .route(
            "/api/v1/nodes/{node_id}/disk-space/{scan_id}/entries",
            get(entries),
        )
        .route("/api/v1/nodes/{node_id}/disk-space/{scan_id}/pause", post(pause))
        .route(
            "/api/v1/nodes/{node_id}/disk-space/{scan_id}/cancel",
            post(cancel),
        )
}

fn db_error() -> OperationsError {
    OperationsError(StatusCode::SERVICE_UNAVAILABLE, "磁盘扫描记录暂不可用")
}
fn response(value: Value) -> Response {
    let mut response = Json(value).into_response();
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}
fn target(state: &OperationsState, id: Uuid) -> Result<Target, OperationsError> {
    load_targets(&state.registry)?
        .into_iter()
        .find(|t| t.node_id == id && !t.disabled)
        .ok_or(OperationsError(
            StatusCode::NOT_FOUND,
            "该机器没有可用的可信 SSH 运维目标",
        ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    #[serde(default = "default_storage")]
    storage: String,
}
// A node-bound capability only this read-only module can mint. It never grants
// a browser permission to use administrator credentials in other operations.
pub(super) struct FullScanGrant(Uuid);
impl FullScanGrant {
    pub(super) fn node_id(&self) -> Uuid { self.0 }
}
fn scan_credentials(state: &OperationsState, target: &Target, grant: &FullScanGrant) -> Result<Credentials, OperationsError> {
    let saved = ssh_vault::load_for_full_disk_scan(&state.registry, target, grant)?;
    let credentials = Credentials {
        username: saved.username, password: saved.password, private_key: saved.key,
        passphrase: saved.passphrase, sudo_password: saved.sudo_password, use_saved: false,
    };
    validate_credentials(&credentials)?;
    Ok(credentials)
}

fn default_storage() -> String {
    "snapshot".into()
}
fn validate_storage(storage: &str) -> Result<(), OperationsError> {
    if matches!(storage, "snapshot" | "realtime") {
        Ok(())
    } else {
        Err(OperationsError::bad_request("扫描模式无效"))
    }
}

async fn list(
    State(state): State<OperationsState>,
    Path(node): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Response, OperationsError> {
    validate_storage(&query.storage)?;
    let target = target(&state, node)?;
    let full = reported_agent_mode(&state.pool, node).await.unwrap_or(false);
    let authorized = full;
    let jobs = if query.storage == "realtime" {
        state.disk_space.live_summaries(node)?
    } else {
        let rows = sqlx::query("SELECT id,mode,scope,root_path,status,started_at,finished_at,progress,message FROM disk_space_scans WHERE node_id=$1 ORDER BY started_at DESC LIMIT 8")
            .bind(node).fetch_all(&state.pool).await.map_err(|_| db_error())?;
        rows.iter().map(summary).collect::<Result<Vec<_>, _>>()?
    };
    let (active_count, node_active_count) = {
        let active = state.disk_space.active.lock().map_err(|_| OperationsError::busy())?;
        (active.len(), active.values().filter(|(existing, _)| *existing == node).count())
    };
    let memory_result_count = state.disk_space.live.lock().map_err(|_| OperationsError::busy())?.len();
    let jobs = jobs.into_iter().map(|value| {
        let id=value["id"].as_str().and_then(|id| Uuid::parse_str(id).ok()).ok_or_else(OperationsError::registry)?;
        scan_control(&state,id,value)
    }).collect::<Result<Vec<_>,OperationsError>>()?;
    Ok(response(
        json!({"scans":jobs,"storage":query.storage,"active_scan":node_active_count>0,"active_count":active_count,"node_active_count":node_active_count,"memory_result_count":memory_result_count,"retention_hours":24,"username":target.default_user,
        "default_path":if target.platform.is_windows(){Some(target.agent_home.clone())}else{target.agent_home.parent().map(FsPath::to_path_buf)},"platform":target.platform, "root_authorized":authorized, "full_access":full,
        "limits":{"seconds":null,"entries":null,"depth":64,"concurrent":MAX_SCANS,"memory_results":MAX_MEMORY_RESULTS,"retention_days":7}}),
    ))
}

// Fixed one-level metadata read, with no caller-supplied commands or traversal.
const HOMES_SCRIPT: &str = r#"
import json, os, sys
with os.scandir('/home') as entries:
    paths = sorted(e.path for e in entries if e.is_dir(follow_symlinks=False))
data = json.dumps(paths, ensure_ascii=True).encode()
if len(data) > 1048576:
    sys.exit(2)
sys.stdout.buffer.write(data)
"#;

async fn homes(
    State(state): State<OperationsState>, Path(node): Path<Uuid>,
) -> Result<Response, OperationsError> {
    let target = target(&state, node)?;
    if target.platform.is_windows() { return Ok(response(json!({"paths":[]}))); }
    // Cache across visitors, and serialize refreshes per host.
    let cache = state.disk_space.homes.lock().map_err(|_| OperationsError::busy())?
        .entry(node).or_insert_with(|| Arc::new(Mutex::new(HomeCache::default()))).clone();
    let paths = tokio::task::spawn_blocking(move || {
        let mut cache = cache.try_lock().map_err(|_| OperationsError::busy())?;
        if let Some((at, paths)) = &cache.value {
            if at.elapsed() < Duration::from_secs(300) { return Ok(paths.clone()); }
        }
        let command = format!("/usr/bin/timeout 15s /usr/bin/python3 -I -B -c {}", shell_quote(HOMES_SCRIPT));
        let bytes = read_homes(&target, &command)?;
        let paths = parse_homes(&bytes)?;
        cache.value = Some((Instant::now(), paths.clone()));
        Ok::<_, OperationsError>(paths)
    }).await.map_err(|_| OperationsError::busy())??;
    Ok(response(json!({"paths":paths})))
}

fn parse_homes(bytes: &[u8]) -> Result<Vec<String>, OperationsError> {
    let invalid = || OperationsError::ssh("无法读取该机器的 /home 目录备选");
    if bytes.len() > MAX_FRAME { return Err(invalid()); }
    let mut paths: Vec<String> = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if paths.iter().any(|p| !p.starts_with("/home/") || p.len() <= 6
        || matches!(&p[6..], "." | "..") || p[6..].contains('/') || p.chars().any(char::is_control)) { return Err(invalid()); }
    paths.sort(); paths.dedup(); Ok(paths)
}

fn read_homes(target: &Target, command: &str) -> Result<Vec<u8>, OperationsError> {
    let error = || OperationsError::ssh("无法列出 /home，请检查该机器的 SSH 连接与目录读取权限");
    // Ordinary trusted transport avoids borrowing administrator credentials or
    // blocking behind a long root scan.
    if target.connection_mode == ssh_transport::Mode::Ssh {
        let session = ssh_transport::primary(target)?;
        session.set_timeout(10_000);
        let mut channel = crate::network::ssh_channel(&session).map_err(|_| error())?;
        channel.handle_extended_data(ExtendedData::Ignore).map_err(|_| error())?;
        channel.exec(command).map_err(|_| error())?;
        let mut bytes = Vec::new();
        (&mut channel).take((MAX_FRAME + 1) as u64).read_to_end(&mut bytes).map_err(|_| error())?;
        if bytes.len() > MAX_FRAME { let _ = channel.close(); return Err(error()); }
        channel.wait_close().map_err(|_| error())?;
        if channel.exit_status().map_err(|_| error())? != 0 { return Err(error()); }
        return Ok(bytes);
    }
    let process = direct_process(target, command)?;
    let mut bounded = Command::new("/usr/bin/timeout");
    bounded.arg("--kill-after=2s").arg("20s").arg(process.get_program()).args(process.get_args());
    let mut child = bounded.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|_| error())?;
    let mut bytes = Vec::new();
    let result = child.stdout.take().ok_or_else(error)?.take((MAX_FRAME + 1) as u64).read_to_end(&mut bytes);
    if result.is_err() || bytes.len() > MAX_FRAME { let _ = child.kill(); let _ = child.wait(); return Err(error()); }
    if !child.wait().map_err(|_| error())?.success() { return Err(error()); }
    Ok(bytes)
}

fn summary(row: &sqlx::postgres::PgRow) -> Result<Value, OperationsError> {
    Ok(
        json!({"storage":"snapshot","id":row.try_get::<Uuid,_>("id").map_err(|_|db_error())?,
        "mode":row.try_get::<String,_>("mode").map_err(|_|db_error())?,
        "scope":row.try_get::<String,_>("scope").map_err(|_|db_error())?,
        "root_path":row.try_get::<String,_>("root_path").map_err(|_|db_error())?,
        "status":row.try_get::<String,_>("status").map_err(|_|db_error())?,
        "started_at":row.try_get::<DateTime<Utc>,_>("started_at").map_err(|_|db_error())?,
        "finished_at":row.try_get::<Option<DateTime<Utc>>,_>("finished_at").map_err(|_|db_error())?,
        "progress":row.try_get::<Value,_>("progress").map_err(|_|db_error())?,
        "message":row.try_get::<String,_>("message").map_err(|_|db_error())?}),
    )
}

async fn record(
    state: &OperationsState,
    node: Uuid,
    id: Uuid,
) -> Result<sqlx::postgres::PgRow, OperationsError> {
    let _ = target(state, node)?;
    let row = sqlx::query("SELECT * FROM disk_space_scans WHERE node_id=$1 AND id=$2")
        .bind(node)
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|_| db_error())?
        .ok_or(OperationsError(
            StatusCode::NOT_FOUND,
            "扫描记录不存在或已到期",
        ))?;
    // Saved directory summaries share the platform's normal read access,
    // regardless of the privilege used to collect them. Root execution and
    // cancellation authorize separately before changing a task.
    Ok(row)
}

async fn detail(
    State(state): State<OperationsState>,
    Path((node, id)): Path<(Uuid, Uuid)>,
) -> Result<Response, OperationsError> {
    let _ = target(&state, node)?;
    if let Some(job) = state.disk_space.live_job(node, id)? {
        return Ok(response(
            scan_control(&state,id,job.lock().map_err(|_| OperationsError::busy())?.detail())?,
        ));
    }
    let row = record(&state, node, id).await?;
    let mut value = summary(&row)?;
    value["result"] = row
        .try_get::<Option<Value>, _>("result")
        .map_err(|_| db_error())?
        .unwrap_or(Value::Null);
    Ok(response(scan_control(&state,id,value)?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryQuery {
    #[serde(default)]
    parent: i64,
    #[serde(default)]
    offset: i64,
    #[serde(default)]
    ascending: bool,
    #[serde(default)]
    search: String,
    #[serde(default = "default_basis")]
    basis: String,
    #[serde(default = "default_sort")]
    sort: String,
}
fn default_basis() -> String {
    "allocated".into()
}
fn default_sort() -> String {
    "size".into()
}

async fn entries(
    State(state): State<OperationsState>,
    Path((node, id)): Path<(Uuid, Uuid)>,
    Query(query): Query<EntryQuery>,
) -> Result<Response, OperationsError> {
    if query.parent < 0
        || query.offset < 0
        || query.search.len() > 128
        || !matches!(query.basis.as_str(), "allocated" | "apparent")
    {
        return Err(OperationsError::bad_request("目录分页参数无效"));
    }
    let order = match query.sort.as_str() {
        "size" | "percent" => query.basis.as_str(),
        "name" => "name",
        "files" => "files",
        _ => return Err(OperationsError::bad_request("排序方式无效")),
    };
    let _ = target(&state, node)?;
    if let Some(job) = state.disk_space.live_job(node, id)? {
        let page = tokio::task::spawn_blocking(move || {
            job.lock()
                .map_err(|_| OperationsError::busy())?
                .page(&query)
        })
        .await
        .map_err(|_| OperationsError::busy())??;
        return Ok(response(page));
    }
    let row = record(&state, node, id).await?;
    if row.try_get::<String, _>("status").map_err(|_| db_error())? == "running" {
        return Err(OperationsError(
            StatusCode::CONFLICT,
            "扫描仍在进行，请完成后查看目录明细",
        ));
    }
    let direction = if query.ascending { "ASC" } else { "DESC" };
    let directory:Value=sqlx::query_scalar("SELECT to_jsonb(e)-'scan_id' FROM disk_space_directories e WHERE scan_id=$1 AND id=$2 AND kind='directory'")
        .bind(id).bind(query.parent).fetch_optional(&state.pool).await.map_err(|_|db_error())?
        .ok_or(OperationsError(StatusCode::NOT_FOUND,"目录不存在或该快照使用旧版格式"))?;
    let ancestors:Vec<Value>=sqlx::query_scalar("WITH RECURSIVE chain AS (SELECT e.*,0 AS depth FROM disk_space_directories e WHERE scan_id=$1 AND id=$2 UNION ALL SELECT p.*,c.depth+1 FROM disk_space_directories p JOIN chain c ON p.scan_id=c.scan_id AND p.id=c.parent WHERE c.depth<64) SELECT to_jsonb(chain)-'scan_id'-'depth' FROM chain ORDER BY depth DESC")
        .bind(id).bind(query.parent).fetch_all(&state.pool).await.map_err(|_|db_error())?;
    // Totals cover every child directory, irrespective of filter/page. The
    // remaining bytes include direct files, links and the directory itself;
    // their individual names are deliberately never persisted.
    let totals = sqlx::query("SELECT COUNT(*) FILTER (WHERE $3='' OR POSITION(lower($3) IN lower(name))>0) AS matched, COALESCE(SUM(allocated),0)::BIGINT AS allocated, COALESCE(SUM(apparent),0)::BIGINT AS apparent, COALESCE(SUM(files),0)::BIGINT AS files FROM disk_space_directories WHERE scan_id=$1 AND parent=$2")
        .bind(id).bind(query.parent).bind(&query.search).fetch_one(&state.pool).await.map_err(|_|db_error())?;
    let total: i64 = totals.try_get("matched").map_err(|_| db_error())?;
    let mut direct = json!({});
    for key in ["allocated", "apparent", "files"] {
        let children: i64 = totals.try_get(key).map_err(|_| db_error())?;
        direct[key] = json!(
            directory[key]
                .as_i64()
                .unwrap_or(0)
                .saturating_sub(children)
                .max(0)
        );
    }
    let statement = format!(
        "SELECT to_jsonb(e)-'scan_id' FROM disk_space_directories e WHERE scan_id=$1 AND parent=$2 AND ($3='' OR POSITION(lower($3) IN lower(name))>0) ORDER BY {order} {direction},id ASC LIMIT 250 OFFSET $4"
    );
    let children: Vec<Value> = sqlx::query_scalar(&statement)
        .bind(id)
        .bind(query.parent)
        .bind(&query.search)
        .bind(query.offset)
        .fetch_all(&state.pool)
        .await
        .map_err(|_| db_error())?;
    let statement = format!(
        "SELECT to_jsonb(e)-'scan_id' FROM disk_space_directories e WHERE scan_id=$1 AND parent=$2 AND {}>0 ORDER BY {} DESC,id ASC LIMIT 40",
        query.basis, query.basis
    );
    let top: Vec<Value> = sqlx::query_scalar(&statement)
        .bind(id)
        .bind(query.parent)
        .fetch_all(&state.pool)
        .await
        .map_err(|_| db_error())?;
    Ok(response(
        json!({"directory":directory,"ancestors":ancestors,"entries":children,"top":top,"direct":direct,
        "total":total,"offset":query.offset,"limit":250}),
    ))
}

async fn cancel(
    State(state): State<OperationsState>,
    Path((node, id)): Path<(Uuid, Uuid)>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response, OperationsError> {
    validate_origin(&headers, &uri)?;
    let _ = target(&state, node)?;
    let mode = if let Some(job) = state.disk_space.live_job(node, id)? {
        job.lock().map_err(|_| OperationsError::busy())?.summary["mode"]
            .as_str()
            .unwrap_or("full")
            .to_owned()
    } else {
        record(&state, node, id)
            .await?
            .try_get::<String, _>("mode")
            .map_err(|_| db_error())?
    };
    if mode == "full" {
        if !reported_agent_mode(&state.pool, node).await? {
            return Err(OperationsError::bad_request("取消 root 扫描需要当前在线的 Full Agent"));
        }
    }
    if let Some((_, stop)) = state
        .disk_space
        .active
        .lock()
        .map_err(|_| OperationsError::busy())?
        .get(&id)
    {
        stop.store(true, Ordering::Relaxed);
    }
    Ok(response(
        json!({"message":"已请求停止扫描；已读取的统计会保留为不完整快照"}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PauseRequest { paused: bool }
async fn pause(
    State(state): State<OperationsState>, Path((node,id)): Path<(Uuid,Uuid)>,
    uri: Uri, headers: HeaderMap, Json(request): Json<PauseRequest>,
) -> Result<Response,OperationsError> {
    validate_origin(&headers,&uri)?;
    let _ = target(&state,node)?;
    // Check the same node-bound record and Full permission as cancellation.
    let mode = if let Some(job)=state.disk_space.live_job(node,id)? {
        job.lock().map_err(|_|OperationsError::busy())?.summary["mode"].as_str().unwrap_or("full").to_owned()
    } else { record(&state,node,id).await?.try_get::<String,_>("mode").map_err(|_|db_error())? };
    if mode=="full" && !reported_agent_mode(&state.pool,node).await? {
        return Err(OperationsError::bad_request("暂停或继续 root 扫描需要当前在线的 Full Agent"));
    }
    let active=state.disk_space.active.lock().map_err(|_|OperationsError::busy())?;
    if !active.get(&id).is_some_and(|(owner,stop)|*owner==node&&!stop.load(Ordering::Relaxed)) {
        return Err(OperationsError(StatusCode::CONFLICT,"扫描已结束或正在取消，不能暂停或继续"));
    }
    let controls=state.disk_space.controls.lock().map_err(|_|OperationsError::busy())?;
    let control=controls.get(&id).ok_or_else(OperationsError::busy)?;
    control.requested.store(request.paused,Ordering::Release);
    Ok(response(json!({"requested_paused":request.paused,"message":if request.paused {"已请求暂停，等待扫描器确认"} else {"已请求继续扫描"}})))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Request {
    path: String,
    mode: String,
    seconds: u64,
    #[serde(default)]
    max_entries: u64,
    #[serde(default = "default_scope")]
    scope: String,
    #[serde(default = "default_storage")]
    storage: String,
    #[serde(default)]
    viewer: Option<Uuid>,
    #[serde(default)]
    review_token: Option<Uuid>,
    #[serde(default)]
    confirmed: bool,
    #[serde(skip)]
    sensitive_authorized: bool,
}
fn default_scope() -> String {
    "directory".into()
}

fn validate(request: &Request) -> Result<(), OperationsError> {
    validate_storage(&request.storage)?;
    if !matches!(request.mode.as_str(), "basic" | "full")
        || !matches!(request.scope.as_str(), "directory" | "all_local")
        || (request.scope == "all_local" && request.path != "/")
        || (request.seconds != 0 && !(5..=86400).contains(&request.seconds))
        || request.max_entries > i64::MAX as u64
        || !(request.path.starts_with('/') || valid_windows_path(&request.path))
        || request.path.len() > 2048
        || request.path.chars().any(char::is_control)
        || request.path.split('/').any(|p| p == ".." || p == ".")
    {
        return Err(OperationsError::bad_request(
            "请选择普通用户/root 和有效绝对路径；时间及条目数为 0 表示不限，有限时限为 5–86400 秒",
        ));
    }
    Ok(())
}

async fn start(
    State(state): State<OperationsState>,
    Path(node): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
    Json(mut request): Json<Request>,
) -> Result<Response, OperationsError> {
    validate_origin(&headers, &uri)?;
    let selected_target=target(&state,node)?;
    safety::normalize(&selected_target,&mut request)?;
    // A refused scan must not consume the limited SSH operation-attempt budget.
    if request.storage == "snapshot" {
        require_storage(storage_available().await?)?;
    } else {
        require_memory()?;
    }
    let target = selected_target;
    let operations_lease = ScanOperationLease::acquire(&state.limits, node)?;
    let enabled: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes WHERE id=$1 AND deleted_at IS NULL AND NOT monitoring_paused)")
        .bind(node).fetch_one(&state.pool).await.map_err(|_|db_error())?;
    if !enabled {
        return Err(OperationsError::bad_request(
            "该机器已暂停监控或已删除，请先恢复",
        ));
    }
    let full = reported_agent_mode(&state.pool, node).await?;
    if request.mode == "full" && !full {
        return Err(OperationsError::bad_request(
            "root 扫描需要当前在线 Agent 为 Full / UID 0",
        ));
    }
    if full { request.mode = "full".into(); }
    if request.mode == "basic" && (target.default_user == "root" || (target.platform.is_windows() && target.agent_mode.as_deref()==Some("full"))) {
        return Err(OperationsError::bad_request(
            "此机器的原 SSH 账户为 root，不能冒充普通用户扫描",
        ));
    }
    // Full hosts always use the real administrator scanner, including requests
    // from older clients that still submit "basic". Basic hosts cannot opt in.
    let credentials = if full {
        Some(scan_credentials(&state, &target, &FullScanGrant(node))?)
    } else { None };
    safety::authorize(&state,&target,&headers,&mut request)?;
    let id = Uuid::new_v4();
    if request.storage == "snapshot" {
        prune(&state.pool, &state.disk_space).await?;
    }
    audit(
        &state.pool,
        node,
        &target.default_user,
        "disk-space.scan",
        "requested",
    )
    .await?;
    let (lease, stop, memory) = state.disk_space.acquire(id, node, &request)?;
    if request.storage == "snapshot" {
        sqlx::query("INSERT INTO disk_space_scans(id,node_id,mode,root_path,scope,status) VALUES($1,$2,$3,$4,$5,'running')")
            .bind(id).bind(node).bind(&request.mode).bind(&request.path).bind(&request.scope).execute(&state.pool).await.map_err(db_query_error)?;
    }
    let runtime = tokio::runtime::Handle::current();
    tokio::spawn(async move {
        let pool = state.pool.clone();
        let scans = state.disk_space.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let _lease = lease;
            let _operations_lease = operations_lease;
            execute(&state, &target, &request, credentials, &stop, &runtime, id)
        })
        .await;
        let (status, result, message) = match worker {
            Ok(Ok(value)) => (
                value["status"].as_str().unwrap_or("partial").to_owned(),
                Some(value.clone()),
                match value["reason"].as_str() {
                    Some("platform_storage_low") => STORAGE_LOW.to_owned(),
                    Some("platform_storage_unknown") => {
                        "无法确认平台存储可用空间，已安全停止扫描；请检查存储保护路径配置"
                            .to_owned()
                    }
                    _ => String::new(),
                },
            ),
            Ok(Err(error)) => ("failed".to_owned(), None, error.1.to_owned()),
            Err(_) => (
                "failed".to_owned(),
                None,
                "扫描任务异常终止，原文件未修改".to_owned(),
            ),
        };
        if let Some(job) = memory {
            if let Ok(mut job) = job.lock() {
                job.finish(&status, result, &message);
            }
        } else {
            if sqlx::query("UPDATE disk_space_scans SET status=$2,result=$3,message=$4,finished_at=NOW() WHERE id=$1 AND status='running'")
                .bind(id).bind(&status).bind(result).bind(message).execute(&pool).await.is_err() {
                tracing::warn!(%id,"disk scan result could not be persisted");
            }
            let _ = prune(&pool, &scans).await;
        }
    });
    Ok(response(json!({"id":id,"status":"running"})))
}

fn invocation(request: &Request, username: &str) -> String {
    let mut value = json!({"path":request.path,"mode":request.mode,"seconds":request.seconds,"max_entries":request.max_entries,"username":username,"scope":request.scope,"approved_sensitive":request.sensitive_authorized});
    if request.storage == "realtime" {
        value["realtime"] = json!(true);
    }
    // The scanner and transport count active time, excluding acknowledged pauses.
    format!(
        "/usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin /usr/bin/ionice -c 3 /usr/bin/python3 -I -B -c {} {}",
        shell_quote(SCRIPT),
        shell_quote(&value.to_string())
    )
}

fn validate_identity(
    value: &Value,
    root: &str,
    mode: &str,
    username: &str,
) -> Result<(), OperationsError> {
    if value["platform"]=="windows" {
        let matches_user=value["username"].as_str().is_some_and(|name|name.eq_ignore_ascii_case(username) || name.rsplit('\\').next().is_some_and(|short|short.eq_ignore_ascii_case(username)));
        if value["root"]!=root || value["native_id"].as_str().is_none_or(|s|!s.starts_with("S-1-")) || value["administrator"].as_bool()!=Some(mode=="full") || (mode=="basic" && !matches_user){return Err(OperationsError::ssh("Windows 扫描执行身份或路径校验失败"));}return Ok(());
    }
    if value["root"] != root
        || value["uid"].as_u64().is_none()
        || (value["uid"] == 0) != (mode == "full")
        || (mode == "basic" && value["username"] != username)
    {
        return Err(OperationsError::ssh("扫描执行身份或目录校验失败"));
    }
    Ok(())
}

fn validate_entry(node: &Value, root: &str, scope: &str) -> Result<u64, OperationsError> {
    let invalid = || OperationsError::ssh("目录条目格式校验失败");
    let id = node["id"]
        .as_u64()
        .filter(|id| *id <= i64::MAX as u64)
        .ok_or_else(invalid)?;
    let name = node["name"].as_str().ok_or_else(invalid)?;
    let filesystem_root = !node["scan_path"].is_null();
    if filesystem_root
        && (scope != "all_local"
            || id == 0
            || node["parent"] != 0
            || node["kind"] != "directory"
            || node["scan_path"].as_str() != Some(name)
            || !(name.starts_with('/') || valid_windows_path(name))
            || name.len() > 2048
            || name.chars().any(char::is_control)
            || name.split('/').any(|part| part == "." || part == ".."))
    {
        return Err(invalid());
    }
    if !matches!(node["kind"].as_str(), Some("directory"))
        || node["incomplete"].as_bool().is_none()
        || node["mtime"].as_i64().is_none()
        || ["allocated", "apparent", "files", "directories"]
            .iter()
            .any(|key| node[*key].as_u64().is_none_or(|n| n > i64::MAX as u64))
        || (!node["issue"].is_null() && node["issue"].as_str().is_none_or(|v| v.len() > 64))
    {
        return Err(invalid());
    }
    if id == 0 {
        if name != root || !node["parent"].is_null() || node["kind"] != "directory" {
            return Err(invalid());
        }
    } else if node["parent"].as_u64().is_none_or(|parent| parent >= id)
        || name.is_empty()
        || (!filesystem_root && (name.len() > 255 || name.contains(['/', '\0'])))
        || name == "."
        || name == ".."
    {
        return Err(invalid());
    }
    Ok(id)
}

struct Output {
    pending: Vec<u8>,
    result: Option<Value>,
    last_progress: Instant,
    next_id: u64,
    authenticated: bool,
    root: String,
    mode: String,
    username: String,
    scope: String,
    storage_checked_at: Instant,
    storage_stop: Option<&'static str>,
    live: Option<live::Job>,
    windows: bool,
}
impl Output {
    fn new(request: &Request, username: &str) -> Self {
        Self {
            windows:false,
            live: None,
            pending: Vec::new(),
            result: None,
            last_progress: Instant::now() - Duration::from_secs(3),
            next_id: 0,
            authenticated: false,
            root: request.path.clone(),
            mode: request.mode.clone(),
            username: username.to_owned(),
            scope: request.scope.clone(),
            storage_checked_at: Instant::now() - Duration::from_secs(3),
            storage_stop: None,
        }
    }

    fn protect_storage(&mut self, runtime: &tokio::runtime::Handle, stop: &AtomicBool) {
        if self.storage_stop.is_some() || self.storage_checked_at.elapsed() < Duration::from_secs(2)
        {
            return;
        }
        self.storage_checked_at = Instant::now();
        self.storage_stop = if self.live.is_some() {
            require_memory().err().map(|_| "platform_memory_low")
        } else {
            match runtime.block_on(storage_available()) {
                Ok(available) if available < STORAGE_RESERVE => Some("platform_storage_low"),
                Ok(_) => None,
                Err(_) => Some("platform_storage_unknown"),
            }
        };
        if self.storage_stop.is_some() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    fn finish(mut self) -> Result<Value, OperationsError> {
        let mut value = self.result.take().ok_or(OperationsError::ssh(
            "扫描没有完整结果；请检查权限、本地文件系统或连接",
        ))?;
        if let Some(reason) = self.storage_stop {
            value["status"] = json!("partial");
            value["reason"] = json!(reason);
        }
        Ok(value)
    }
    fn feed(
        &mut self,
        bytes: &[u8],
        state: &OperationsState,
        runtime: &tokio::runtime::Handle,
        id: Uuid,
    ) -> Result<(), OperationsError> {
        self.pending.extend_from_slice(bytes);
        if self.pending.len() > MAX_FRAME {
            return Err(OperationsError::ssh("扫描单批响应格式超限"));
        }
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let value: Value = serde_json::from_slice(&line)
                .map_err(|_| OperationsError::ssh("扫描响应格式无效"))?;
            match value["type"].as_str() {
                Some("metadata") if !self.authenticated => {
                    if (value["platform"]=="windows")!=self.windows{return Err(OperationsError::ssh("扫描平台不匹配"));}
                    validate_identity(&value, &self.root, &self.mode, &self.username)?;
                    if value["scope"] != self.scope {
                        return Err(OperationsError::ssh("扫描范围不匹配"));
                    }
                    if let Some(job) = &self.live {
                        job.lock()
                            .map_err(|_| OperationsError::busy())?
                            .metadata(&value);
                    }
                    self.authenticated = true;
                }
                Some("control") if self.authenticated && self.result.is_none() => {
                    let paused=value["paused"].as_bool().ok_or_else(||OperationsError::ssh("扫描控制响应无效"))?;
                    if let Some(control)=state.disk_space.controls.lock().map_err(|_|OperationsError::busy())?.get(&id) { control.acknowledge(paused); }
                    if let Some(job)=&self.live { job.lock().map_err(|_|OperationsError::busy())?.control_changed(); }
                }
                Some("entries") if self.authenticated && self.result.is_none() => {
                    let nodes = value["nodes"]
                        .as_array()
                        .filter(|n| !n.is_empty() && n.len() <= 256)
                        .ok_or(OperationsError::ssh("扫描批次格式错误"))?;
                    let mut batch = HashSet::new();
                    for node in nodes {
                        let entry = validate_entry(node, &self.root, &self.scope)?;
                        if entry > self.next_id || !batch.insert(entry) {
                            return Err(OperationsError::ssh("扫描条目顺序无效"));
                        }
                        if entry == self.next_id {
                            self.next_id += 1;
                        }
                    }
                    if let Some(job) = &self.live {
                        job.lock()
                            .map_err(|_| OperationsError::busy())?
                            .insert(nodes)?;
                    } else {
                        let written=runtime.block_on(sqlx::query("INSERT INTO disk_space_directories(scan_id,id,parent,name,kind,allocated,apparent,files,directories,issue,incomplete,mtime,scan_path) SELECT $1,x.* FROM jsonb_to_recordset($2) AS x(id BIGINT,parent BIGINT,name TEXT,kind TEXT,allocated BIGINT,apparent BIGINT,files BIGINT,directories BIGINT,issue TEXT,incomplete BOOLEAN,mtime BIGINT,scan_path TEXT) ON CONFLICT(scan_id,id) DO UPDATE SET allocated=EXCLUDED.allocated,apparent=EXCLUDED.apparent,files=EXCLUDED.files,directories=EXCLUDED.directories,issue=EXCLUDED.issue,incomplete=EXCLUDED.incomplete WHERE disk_space_directories.parent IS NOT DISTINCT FROM EXCLUDED.parent AND disk_space_directories.name=EXCLUDED.name AND disk_space_directories.kind=EXCLUDED.kind AND disk_space_directories.scan_path IS NOT DISTINCT FROM EXCLUDED.scan_path")
                        .bind(id).bind(&value["nodes"]).execute(&state.pool)).map_err(db_query_error)?;
                        if written.rows_affected() != nodes.len() as u64 {
                            return Err(OperationsError::ssh("目录条目身份发生变化"));
                        }
                    }
                }
                Some("progress") if self.last_progress.elapsed() >= Duration::from_secs(1) => {
                    let progress = json!({"estimated_percent":value["estimated_percent"].as_f64().filter(|n|n.is_finite()).map(|n|n.clamp(0.0,99.9)),"visited":value["visited"].as_u64().unwrap_or(0),
                        "errors":value["errors"].as_u64().unwrap_or(0),"elapsed_seconds":value["elapsed_seconds"].as_f64().unwrap_or(0.0)});
                    if let Some(job) = &self.live {
                        job.lock()
                            .map_err(|_| OperationsError::busy())?
                            .progress(&value);
                    } else {
                        runtime.block_on(sqlx::query("UPDATE disk_space_scans SET progress=$2 WHERE id=$1 AND status='running'").bind(id).bind(progress).execute(&state.pool)).map_err(db_query_error)?;
                    }
                    self.last_progress = Instant::now();
                }
                Some("progress") => {}
                Some("result") if self.authenticated && self.result.is_none() => {
                    if (value["platform"]=="windows")!=self.windows{return Err(OperationsError::ssh("扫描平台不匹配"));}
                    validate_identity(&value, &self.root, &self.mode, &self.username)?;
                    if value["paged"] != true
                        || value["scope"] != self.scope
                        || value["detail_level"] != "directories"
                        || value["saved_directories"].as_u64() != Some(self.next_id)
                        || value["visited"].as_u64().is_none_or(|n| n < self.next_id)
                        || !matches!(
                            value["status"].as_str(),
                            Some("complete" | "partial" | "cancelled")
                        )
                        || value["nodes"].as_array().is_none_or(|n| n.len() != 1)
                        || validate_entry(&value["nodes"][0], &self.root, &self.scope)? != 0
                    {
                        return Err(OperationsError::ssh("扫描汇总不完整"));
                    }
                    self.result = Some(value);
                }
                _ => {
                    return Err(OperationsError::ssh(
                        "目录不可读取、身份不匹配或文件系统不支持；仅支持本地磁盘且路径不得包含符号链接",
                    ));
                }
            }
        }
        Ok(())
    }
}

fn execute(
    state: &OperationsState,
    target: &Target,
    request: &Request,
    credentials: Option<Credentials>,
    stop: &AtomicBool,
    runtime: &tokio::runtime::Handle,
    id: Uuid,
) -> Result<Value, OperationsError> {
    let control=state.disk_space.controls.lock().map_err(|_|OperationsError::busy())?.get(&id).cloned().ok_or_else(OperationsError::registry)?;
    let command = invocation(request, &target.default_user);
    let mut output = Output::new(request, &target.default_user);
    output.live = state.disk_space.live_job(target.node_id, id)?;
    output.windows=target.platform.is_windows();
    if target.platform.is_windows() || credentials.is_some() || target.connection_mode == ssh_transport::Mode::Ssh {
        let cancel = Cancellation::default();
        // Each scan owns a session: libssh2 blocking mode and scanner controls
        // must never be shared with another scan or the monitoring transport.
        let auth = if let Some(auth) = &credentials {
            ssh_transport::Auth { username: auth.username.clone(), password: auth.password.clone(), key: auth.private_key.clone(), passphrase: auth.passphrase.clone() }
        } else {
            let saved = ssh_vault::load(&state.registry, target)?;
            if !saved.enabled { return Err(OperationsError::ssh("扫描 SSH 凭据已停用，请重新连接机器")); }
            saved.auth()
        };
        let session = ssh_transport::scan(target, &auth, &cancel)?;
        drop(auth);
        session.set_timeout(10_000);
        let mut channel = crate::network::ssh_channel(&session)
            .map_err(|_| OperationsError::ssh("扫描 SSH 通道不可用"))?;
        channel
            .handle_extended_data(ExtendedData::Ignore)
            .map_err(|_| OperationsError::ssh("扫描 SSH 通道不可用"))?;
        let sudo = !target.platform.is_windows() && credentials.as_ref().is_some_and(|c| c.username != "root");
        channel
            .exec(&if target.platform.is_windows(){windows_invocation(&target.pwsh_path,false)?}else if sudo {
                format!("/usr/bin/sudo -k -S -p '' {command}")
            } else {
                command
            })
            .map_err(|_| OperationsError::ssh("扫描命令启动失败"))?;
        if sudo {
            let auth = credentials.as_ref().ok_or_else(OperationsError::registry)?;
            channel
                .write_all(auth.sudo_secret().as_bytes())
                .and_then(|()| channel.write_all(b"\n"))
                .map_err(|_| OperationsError::ssh("管理员身份验证失败"))?;
        }
        if target.platform.is_windows(){
            use base64::Engine;
            let value=json!({"path":request.path,"scope":request.scope,"mode":request.mode,"seconds":request.seconds,"max_entries":request.max_entries,"approved_sensitive":request.sensitive_authorized});
            let script=format!("& {{\n$request='{}' | ConvertFrom-Json\n{}\n}}\n",value.to_string().replace('\'',"''"),include_str!("../../deploy/windows/scan-disk-space.ps1"));
            let encoded=base64::engine::general_purpose::STANDARD.encode(script.as_bytes());
            channel.write_all(encoded.as_bytes()).and_then(|()|channel.write_all(b"\n")).map_err(|_|OperationsError::ssh("Windows 扫描脚本传输失败"))?;
        }
        drop(credentials);
        session.set_blocking(false);
        let started = Instant::now();
        let mut stopping = false;
        let mut sent_pause=false;
        let mut cancelled_at = None;
        let mut buffer = [0_u8; 32768];
        loop {
            output.protect_storage(runtime, stop);
            let wanted=control.requested.load(Ordering::Acquire);
            if !stop.load(Ordering::Relaxed) && wanted!=sent_pause {
                match channel.write(if wanted {b"p"} else {b"r"}) {
                    Ok(1)=>sent_pause=wanted,
                    Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>{},
                    _=>return Err(OperationsError::ssh("扫描控制通道中断")),
                }
            }
            if stop.load(Ordering::Relaxed) && !stopping {
                cancelled_at.get_or_insert_with(Instant::now);
                match channel.write(b"x") {
                    Ok(1) => stopping = true,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    _ => break,
                }
            }
            if cancelled_at.is_some_and(|at: Instant| at.elapsed() > Duration::from_secs(30)) {
                let _ = channel.close();
                return Err(OperationsError::ssh("取消扫描后远端未响应，已关闭扫描通道"));
            }
            if request.seconds > 0 && control.elapsed(started) > Duration::from_secs(request.seconds + 45)
            {
                let _ = channel.close();
                return Err(OperationsError::ssh("扫描达到所选时限且未正常结束"));
            }
            match channel.read(&mut buffer) {
                Ok(0) if channel.eof() => break,
                Ok(n) => output.feed(&buffer[..n], state, runtime, id)?,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Err(OperationsError::ssh("扫描连接中断，可手动重试")),
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        session.set_blocking(true);
        session.set_timeout(2000);
        let _ = channel.send_eof();
        let _ = channel.wait_close();
    } else {
        execute_process(
            state,
            target,
            &command,
            request.seconds,
            stop,
            runtime,
            id,
            &mut output,
        )?;
    }
    output.finish()
}

fn direct_process(target: &Target, command: &str) -> Result<Command, OperationsError> {
    let alias = target
        .ssh_alias
        .as_deref()
        .ok_or_else(OperationsError::registry)?;
    let process = if alias == "local" {
        // The fixed scanner verifies effective UID and getpwuid().pw_name before
        // touching the requested directory; environment USER is not an identity.
        let mut p = Command::new("/bin/sh");
        p.args(["-c", command]);
        p
    } else {
        // The operator-owned alias supplies login keys, but cannot redirect this
        // read operation to a different host/user or bypass the pinned host key.
        let mut p = Command::new("/usr/bin/ssh");
        p.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "ServerAliveInterval=30",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "ClearAllForwardings=yes",
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
            "-o",
            "GlobalKnownHostsFile=/dev/null",
        ])
        .arg("-o")
        .arg(format!(
            "UserKnownHostsFile={}",
            target.known_hosts.display()
        ))
        .arg("-o")
        .arg(format!("HostName={}", target.host))
        .arg("-p")
        .arg(target.port.to_string())
        .arg("-l")
        .arg(&target.default_user)
        .arg(alias)
        .arg(command);
        p
    };
    Ok(process)
}

fn execute_process(
    state: &OperationsState,
    target: &Target,
    command: &str,
    seconds: u64,
    stop: &AtomicBool,
    runtime: &tokio::runtime::Handle,
    id: Uuid,
    output: &mut Output,
) -> Result<(), OperationsError> {
    let alias = target.ssh_alias.as_deref().ok_or_else(OperationsError::registry)?;
    let mut process = direct_process(target, command)?;
    let network = if alias == "local" {
        None
    } else {
        Some(easymanage_runtime::network::counter(
            easymanage_protocol::NetworkGroup::SshConnect,
        ))
    };
    let mut connection_attempt = network.map(easymanage_runtime::network::Counters::begin);
    let _connection = network.map(easymanage_runtime::network::Counters::connection);
    let channel_counters = network
        .map(|_| easymanage_runtime::network::counter(easymanage_protocol::NetworkGroup::SshExec));
    let mut channel_attempt = channel_counters.map(easymanage_runtime::network::Counters::begin);
    let mut child = process
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| OperationsError::ssh("扫描进程启动失败"))?;
    let mut input = child.stdin.take();
    let mut stream = child.stdout.take().ok_or_else(OperationsError::registry)?;
    let (sender, receiver) = std::sync::mpsc::sync_channel::<Vec<u8>>(4);
    let reader = std::thread::spawn(move || {
        let mut buffer = [0_u8; 32768];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if sender.send(buffer[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let control=state.disk_space.controls.lock().map_err(|_|OperationsError::busy())?.get(&id).cloned().ok_or_else(OperationsError::registry)?;
    let mut sent_pause=false;
    let started = Instant::now();
    let mut cancelled_at = None;
    let result = (|| {
        loop {
            output.protect_storage(runtime, stop);
            let wanted=control.requested.load(Ordering::Acquire);
            if !stop.load(Ordering::Relaxed) && wanted!=sent_pause {
                if let Some(input)=&mut input {input.write_all(if wanted {b"p"} else {b"r"}).map_err(|_|OperationsError::ssh("扫描控制通道中断"))?;}
                sent_pause=wanted;
            }
            if stop.load(Ordering::Relaxed) {
                input.take();
                cancelled_at.get_or_insert_with(Instant::now);
            }
            if cancelled_at.is_some_and(|at: Instant| at.elapsed() > Duration::from_secs(30)) {
                return Err(OperationsError::ssh("取消扫描后远端未响应，已关闭扫描进程"));
            }
            if seconds > 0 && control.elapsed(started) > Duration::from_secs(seconds + 45) {
                return Err(OperationsError::ssh("扫描达到所选时限且未正常结束"));
            }
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(bytes) => {
                    if let Some(counter) = channel_counters {
                        counter.receive(bytes.len());
                    }
                    output.feed(&bytes, state, runtime, id)?;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        Ok(())
    })();
    drop(input);
    drop(receiver);
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let ok = result.is_ok() && output.result.is_some();
    if let Some(attempt) = &mut connection_attempt {
        attempt.finish(ok);
    }
    if let Some(attempt) = &mut channel_attempt {
        attempt.finish(ok);
    }
    if let Some(counter) = channel_counters {
        counter.submit(command.len());
    }
    // Do not wait on a reader held by an unresponsive remote filesystem.
    drop(reader);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn storage_guard_keeps_unlimited_limits_and_preserves_cancelled_partial_result() {
        assert_eq!(parse_available(b"524288:4096\n"), Some(STORAGE_RESERVE));
        assert_eq!(parse_available(b"18446744073709551615:4096"), None);
        assert_eq!(parse_available(b"not-a-size"), None);
        assert!(require_storage(STORAGE_RESERVE).is_ok());
        assert_eq!(
            require_storage(STORAGE_RESERVE - 1).unwrap_err().0,
            StatusCode::INSUFFICIENT_STORAGE
        );
        let request = Request {
            path: "/fixture".into(),
            mode: "basic".into(),
            scope: "directory".into(),
            seconds: 0,
            storage: default_storage(),
            viewer: None,
            max_entries: 0,
            ..Request::default()
        };
        assert!(validate(&request).is_ok());
        let mut output = Output::new(&request, "fixture");
        output.storage_stop = Some("platform_storage_low");
        let partial = json!({"status":"cancelled","reason":"cancelled","visited":1200,"nodes":[{"id":0,"allocated":9000,"incomplete":true}]});
        output.result = Some(partial.clone());
        let result = output.finish().unwrap();
        assert_eq!(result["status"], "partial");
        assert_eq!(result["reason"], "platform_storage_low");
        assert_eq!(result["nodes"], partial["nodes"]);
        assert_eq!(result["visited"], 1200);
    }
    #[test]
    fn home_choices_are_complete_sorted_and_bound_to_home_children() {
        let paths = (0..40).rev().map(|i| format!("/home/user{i:02}")).collect::<Vec<_>>();
        assert_eq!(parse_homes(&serde_json::to_vec(&paths).unwrap()).unwrap().len(), 40);
        assert_eq!(parse_homes(br#"["/home/z","/home/a","/home/z"]"#).unwrap(), vec!["/home/a", "/home/z"]);
        for invalid in [br#"["/etc"]"#.as_slice(), br#"["/home/a/b"]"#, br#"["/home/.."]"#, br#"["/home/a\n"]"#, br#"{}"#] {
            assert!(parse_homes(invalid).is_err());
        }
    }

    #[test]
    fn full_scan_grant_never_authorizes_general_admin_operations() {
        use std::os::unix::fs::PermissionsExt;
        use std::fs;
        let directory = std::env::temp_dir().join(format!("full-scan-vault-{}", Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let known = directory.join("known_hosts");
        fs::write(&known, "synthetic pinned host key").unwrap();
        fs::set_permissions(&known, fs::Permissions::from_mode(0o600)).unwrap();
        let registry = directory.join("targets.json");
        let mut target: Target = serde_json::from_value(json!({"node_id":Uuid::new_v4(),"hostname":"fixture","host":"10.222.0.9","port":2222,
            "default_user":"fixture","agent_home":"/home/fixture/.easymanage","known_hosts":known})).unwrap();
        let credentials = Credentials {username:"fixture".into(), password:Zeroizing::new("synthetic-password".into()), ..Credentials::default()};
        ssh_vault::save_admin(&registry, &target, &credentials).unwrap();
        let grant = FullScanGrant(target.node_id);
        assert!(ssh_vault::load_for_full_disk_scan(&registry, &target, &grant).is_ok());
        assert!(ssh_vault::load_admin(&registry, &target, &HeaderMap::new()).is_err());
        assert!(ssh_vault::load_for_full_disk_scan(&registry, &target, &FullScanGrant(Uuid::new_v4())).is_err());
        let old_host = target.host.clone();
        target.host = "10.222.0.10".parse().unwrap();
        assert!(ssh_vault::load_for_full_disk_scan(&registry, &target, &grant).is_err());
        target.host = old_host;
        fs::remove_file(directory.join("ssh-vault").join(format!("admin-{}.enc", target.node_id))).unwrap();
        let auth = ssh_transport::Auth {username:"fixture".into(),password:credentials.password.clone(),key:Zeroizing::new(String::new()),passphrase:Zeroizing::new(String::new())};
        ssh_vault::save(&registry, &target, &auth, "", true).unwrap();
        assert!(ssh_vault::load_for_full_disk_scan(&registry, &target, &grant).is_err());
        target.agent_mode = Some("full".into());
        assert!(ssh_vault::load_for_full_disk_scan(&registry, &target, &grant).is_ok());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn bounds_and_shell_arguments_are_separate() {
        for path in ["relative", "/../etc", "/a/./b", "/a\ncmd"] {
            assert!(
                validate(&Request {
                    path: path.into(),
                    mode: "basic".into(),
                    seconds: 60,
                    storage: default_storage(),
                    viewer: None,
                    max_entries: 0,
                    scope: default_scope(),
                    ..Request::default()
                })
                .is_err()
            );
        }
        let request = Request {
            path: "/tmp/a';$(echo nope)".into(),
            mode: "basic".into(),
            seconds: 0,
            storage: default_storage(),
            viewer: None,
            max_entries: 0,
            scope: default_scope(),
            ..Request::default()
        };
        assert!(validate(&request).is_ok());
        assert!(!invocation(&request, "fixture").contains("/usr/bin/timeout"));
        assert!(invocation(&request,"fixture").ends_with(&shell_quote(&json!({"path":request.path,"mode":"basic","seconds":0,"max_entries":0,"username":"fixture","scope":"directory","approved_sensitive":false}).to_string())));
        assert!(
            validate(&Request {
                scope: "all_local".into(),
                ..request
            })
            .is_err()
        );
        let all = Request {
            path: "/".into(),
            mode: "basic".into(),
            scope: "all_local".into(),
            seconds: 0,
            storage: default_storage(),
            viewer: None,
            max_entries: 0,
            ..Request::default()
        };
        assert!(validate(&all).is_ok());
    }
    #[test]
    fn filesystem_roots_require_explicit_scope_and_stable_path() {
        let node = json!({"id":1,"parent":0,"name":"/data/disk","kind":"directory","scan_path":"/data/disk",
            "allocated":0,"apparent":0,"files":0,"directories":0,"issue":null,"incomplete":false,"mtime":0});
        assert!(validate_entry(&node, "/", "all_local").is_ok());
        assert!(validate_entry(&node, "/", "directory").is_err());
        let mut changed = node.clone();
        changed["scan_path"] = json!("/other");
        assert!(validate_entry(&changed, "/", "all_local").is_err());
        let mut nested = node.clone();
        nested["parent"] = json!(1);
        nested["id"] = json!(2);
        assert!(validate_entry(&nested, "/", "all_local").is_err());
        // Enforce directory-only storage at the transport boundary as well as
        // the database; a scanner regression cannot restore per-file storage.
        let mut file = node.clone();
        file["kind"] = json!("file");
        file["name"] = json!("private-file");
        file["scan_path"] = Value::Null;
        assert!(validate_entry(&file, "/", "directory").is_err());
    }
    fn realtime_request() -> Request {
        serde_json::from_value(json!({"mode":"basic","path":"/tmp/fixture","seconds":0,"storage":"realtime"})).unwrap()
    }
    #[test]
    fn ten_same_host_scans_are_independent_and_eleventh_is_refused() {
        let state = Scans::default();
        state.ready.store(true, Ordering::Release);
        let node = Uuid::new_v4();
        let request = realtime_request();
        let mut leases = Vec::new();
        for _ in 0..MAX_SCANS { leases.push(state.acquire(Uuid::new_v4(), node, &request).unwrap()); }
        state.controls.lock().unwrap().values().next().unwrap().requested.store(true,Ordering::Release);
        assert_eq!(state.active.lock().unwrap().len(),10);
        assert!(state.acquire(Uuid::new_v4(), Uuid::new_v4(), &request).is_err());
        leases.pop();
        assert!(state.acquire(Uuid::new_v4(), node, &request).is_ok());
        assert_eq!(state.controls.lock().unwrap().len(),9);
    }
    #[test]
    fn simultaneous_admissions_never_exceed_ten() {
        let state = Scans::default(); state.ready.store(true, Ordering::Release);
        let node=Uuid::new_v4();
        let barrier=Arc::new(std::sync::Barrier::new(21));
        let count=Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut threads=Vec::new();
        for _ in 0..20 {
            let (state,barrier,count)=(state.clone(),barrier.clone(),count.clone());
            threads.push(std::thread::spawn(move || {
                let lease=state.acquire(Uuid::new_v4(),node,&realtime_request());
                if lease.is_ok() { count.fetch_add(1,Ordering::SeqCst); }
                barrier.wait(); drop(lease);
            }));
        }
        barrier.wait();
        for thread in threads { thread.join().unwrap(); }
        assert_eq!(count.load(Ordering::SeqCst),MAX_SCANS);
        assert!(state.active.lock().unwrap().is_empty());
    }
    #[test]
    fn cache_caps_at_hundred_and_evicts_only_oldest_finished_result() {
        let state=Scans::default(); state.ready.store(true,Ordering::Release);
        let node=Uuid::new_v4(); let request=realtime_request();
        let paused_id=Uuid::new_v4(); let held=state.acquire(paused_id,node,&request).unwrap();
        state.controls.lock().unwrap()[&paused_id].acknowledge(true);
        let oldest=Uuid::new_v4();
        for i in 0..99 {
            let id=if i==0 {oldest} else {Uuid::new_v4()};
            let (lease,_,job)=state.acquire(id,node,&request).unwrap();
            let job=job.unwrap();
            job.lock().unwrap().finish("complete",None,"");
            if i==0 { job.lock().unwrap().summary["finished_at"]=json!("2000-01-01T00:00:00Z"); }
            drop(lease);
        }
        let extra=Uuid::new_v4(); let newest=state.acquire(extra,Uuid::new_v4(),&request).unwrap();
        let jobs=state.live.lock().unwrap();
        assert_eq!(jobs.len(),100); assert!(!jobs.contains_key(&oldest));
        assert!(jobs.contains_key(&paused_id)); assert!(jobs.contains_key(&extra));
        assert!(!held.1.load(Ordering::Acquire));
        drop(jobs);drop(newest);drop(held);
    }
    #[test]
    fn read_scans_preserve_exclusive_management_guard_and_separate_core_budget() {
        let limits=Arc::new(Mutex::new(Limits::default())); let node=Uuid::new_v4();
        let scans:Vec<_>=(0..10).map(|_|ScanOperationLease::acquire(&limits,node).unwrap()).collect();
        assert!(acquire_node_lease(&limits,node,true,false).is_err());
        let ordinary=acquire_node_lease(&limits,node,false,false).unwrap();
        drop(scans); drop(ordinary);
        let mutation=acquire_node_lease(&limits,node,true,false).unwrap();
        assert!(ScanOperationLease::acquire(&limits,node).is_err());
        drop(mutation);
        assert!(ScanOperationLease::acquire(&limits,node).is_ok());
    }
}
