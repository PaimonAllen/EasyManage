//! Browser operations are separate from unauthenticated, read-only monitoring.
//! Targets come only from the operator-owned registry. Credentials are transient,
//! SSH host keys are verified before authentication, and terminal contents are
//! never written to the application log or audit table.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use ssh2::{CheckResult, ExtendedData, KnownHostFileKind, Session};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpStream};
use std::path::{Component, Path as FsPath, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

#[path = "machine_management.rs"]
#[cfg(target_os = "linux")]
mod machines;

#[path = "ssh_transport.rs"]
mod ssh_transport;

#[path = "platform_update.rs"]
#[cfg(target_os = "linux")]
mod platform_update;

const MAX_FRAME_BYTES: usize = 16 * 1024;
const SSH_TIMEOUT: Duration = Duration::from_secs(10);
const AUTH_TIMEOUT: Duration = Duration::from_secs(15);
const IDLE_TIMEOUT: Duration = Duration::from_mins(10);
const MAX_SESSION_TIME: Duration = Duration::from_hours(1);
const ELEVATION_TIMEOUT: Duration = Duration::from_secs(90);
const PROMOTION_SCRIPT: &str = include_str!("../../deploy/linux/operations/promote-agent.sh");
const FULL_RESTART_SCRIPT: &str =
    include_str!("../../deploy/linux/operations/restart-full-agent.sh");
const FORCE_RESTART_SCRIPT: &str =
    include_str!("../../deploy/linux/operations/force-restart-service.sh");
const USER_SCRIPT: &str = include_str!("../../deploy/linux/operations/manage-user.sh");

#[derive(Clone)]
struct OperationsState {
    pool: PgPool,
    initialized: crate::database::Initialized,
    registry: PathBuf,
    limits: Arc<Mutex<Limits>>,
    #[cfg(target_os = "linux")]
    machines: machines::Tasks,
    #[cfg(target_os = "linux")]
    config: Arc<crate::config::ServerConfig>,
    #[cfg(target_os = "linux")]
    platform_update_cache: platform_update::Cache,
}

#[derive(Default)]
struct Limits {
    active: HashMap<Uuid, usize>,
    elevating: HashSet<Uuid>,
    attempts: VecDeque<(Instant, Uuid)>,
}

struct Lease {
    limits: Arc<Mutex<Limits>>,
    node_id: Uuid,
    elevation: bool,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut limits) = self.limits.lock() {
            if self.elevation {
                limits.elevating.remove(&self.node_id);
            }
            if let Some(count) = limits.active.get_mut(&self.node_id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    limits.active.remove(&self.node_id);
                }
            }
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct Target {
    node_id: Uuid,
    hostname: String,
    host: IpAddr,
    #[serde(default = "default_port")]
    port: u16,
    default_user: String,
    agent_home: PathBuf,
    known_hosts: PathBuf,
    #[serde(default)]
    agent_sha256: Option<String>,
    #[serde(default)]
    server_url: Option<String>,
    #[serde(default)]
    ca_sha256: Option<String>,
    #[serde(default)]
    release_sha256: Option<String>,
    #[serde(default, rename = "ssh_target")]
    ssh_alias: Option<String>,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    connection_mode: ssh_transport::Mode,
    #[serde(default)]
    ssh_remote_port: Option<u16>,
    #[serde(default)]
    agent_mode: Option<String>,
}

impl Target {
    fn elevation_available(&self) -> bool {
        self.default_user != "root"
            && self.agent_sha256.is_some()
            && self.server_url.is_some()
            && self.ca_sha256.is_some()
    }

    fn update_available(&self) -> bool {
        self.update_state() == "available"
    }

    fn update_state(&self) -> &'static str {
        if self.release_sha256.is_none() {
            return "not_published";
        }
        if self.ssh_alias.is_none() && self.connection_mode != ssh_transport::Mode::Ssh {
            return "untrusted_target";
        }
        match (&self.release_sha256, &self.agent_sha256) {
            (Some(release), Some(installed)) if release.eq_ignore_ascii_case(installed) => {
                "current"
            }
            (Some(_), Some(_)) => "available",
            _ => "unknown_installed",
        }
    }

    fn restart_available(&self) -> bool {
        (self.default_user != "root" && self.ssh_alias.is_some()) || self.connection_mode == ssh_transport::Mode::Ssh
    }
}

const fn default_port() -> u16 {
    22
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Registry {
    Wrapped { targets: Vec<Target> },
    List(Vec<Target>),
}

#[derive(Serialize)]
struct PublicTarget {
    node_id: Uuid,
    hostname: String,
    host: IpAddr,
    port: u16,
    default_user: String,
    elevation_available: bool,
    update_available: bool,
    update_state: &'static str,
    restart_available: bool,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateRequest {
    #[serde(default)]
    username: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_secret")]
    password: Option<Zeroizing<String>>,
}

fn deserialize_optional_secret<'de, D>(
    deserializer: D,
) -> Result<Option<Zeroizing<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(|value| value.map(Zeroizing::new))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    username: String,
    #[serde(deserialize_with = "deserialize_secret")]
    password: Zeroizing<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserActionRequest {
    username: String,
    #[serde(deserialize_with = "deserialize_secret")]
    password: Zeroizing<String>,
    action: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    shell: String,
    #[serde(default)]
    groups: Option<Vec<String>>,
    #[serde(default)]
    confirmation: String,
}

struct UserOperation {
    action: String,
    subject: String,
    display_name: String,
    shell: String,
    groups: Option<Vec<String>>,
    confirmation: String,
}

fn deserialize_secret<'de, D>(deserializer: D) -> Result<Zeroizing<String>, D::Error>
where
    D: Deserializer<'de>,
{
    String::deserialize(deserializer).map(Zeroizing::new)
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum BrowserMessage {
    Auth {
        username: String,
        #[serde(deserialize_with = "deserialize_secret")]
        password: Zeroizing<String>,
        #[serde(default = "default_cols")]
        cols: u32,
        #[serde(default = "default_rows")]
        rows: u32,
    },
    Input {
        data: String,
    },
    Resize {
        cols: u32,
        rows: u32,
    },
}

const fn default_cols() -> u32 {
    100
}

const fn default_rows() -> u32 {
    30
}

enum TerminalInput {
    Data(Zeroizing<Vec<u8>>),
    Resize(u32, u32),
}

enum TerminalEvent {
    Ready,
    Output(String),
    Error(&'static str),
    Closed,
}

/// Merge the returned router with the API router, without the API's state.
pub fn router(
    pool: PgPool,
    initialized: crate::database::Initialized,
    config: Arc<crate::config::ServerConfig>,
) -> Router {
    ssh_transport::maintenance();
    #[cfg(not(target_os = "linux"))]
    drop(config);
    let state = OperationsState {
        pool,
        initialized,
        registry: std::env::var_os("EASYMANAGE_OPERATIONS_TARGETS").map_or_else(
            || PathBuf::from("/etc/easymanage/targets.json"),
            PathBuf::from,
        ),
        limits: Arc::new(Mutex::new(Limits::default())),
        #[cfg(target_os = "linux")]
        machines: machines::Tasks::default(),
        #[cfg(target_os = "linux")]
        config,
        #[cfg(target_os = "linux")]
        platform_update_cache: platform_update::Cache::default(),
    };
    Router::new()
        .route("/api/v1/operations/targets", get(list_targets))
        .route("/api/v1/operations/recovery", get(recovery_status))
        .route(
            "/api/v1/platform/force-restart",
            post(force_restart_platform),
        )
        .route(
            "/api/v1/nodes/{node_id}/agent/force-restart",
            post(force_restart_agent),
        )
        .route("/api/v1/nodes/{node_id}/elevate", post(elevate))
        .route("/api/v1/nodes/{node_id}/terminal", get(terminal))
        .route("/api/v1/nodes/{node_id}/users/manage", post(manage_user))
        .route("/api/v1/nodes/{node_id}/agent/update", post(update_agent))
        .route("/api/v1/nodes/{node_id}/agent/restart", post(restart_agent))
        .merge(machine_routes())
        .merge(platform_update_routes())
        .layer(DefaultBodyLimit::max(MAX_FRAME_BYTES))
        .with_state(state)
}

fn platform_update_routes() -> Router<OperationsState> {
    #[cfg(target_os = "linux")]
    {
        platform_update::router()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Router::new()
    }
}

fn machine_routes() -> Router<OperationsState> {
    #[cfg(target_os = "linux")]
    {
        machines::router()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Router::new().route(
            "/api/v1/machines",
            get(async || {
                (
                    StatusCode::NOT_IMPLEMENTED,
                    Json(json!({"message":"当前机器部署后端仅支持 Linux 平台"})),
                )
            }),
        )
    }
}

fn platform_target(state: &OperationsState) -> Result<Target, OperationsError> {
    let owner = std::env::var("USER").unwrap_or_default();
    let mut matches = load_targets(&state.registry)?.into_iter().filter(|target| {
        target.ssh_alias.as_deref() == Some("local") && target.default_user == owner
    });
    let target = matches
        .next()
        .ok_or(OperationsError::bad_request("平台本机可信 SSH 目标未配置"))?;
    if matches.next().is_some() {
        return Err(OperationsError::registry());
    }
    Ok(target)
}

async fn recovery_status(State(state): State<OperationsState>) -> Json<serde_json::Value> {
    let database_healthy = state.initialized.load(Ordering::Acquire)
        && matches!(
            tokio::time::timeout(
                Duration::from_secs(2),
                sqlx::query("SELECT 1").execute(&state.pool)
            )
            .await,
            Ok(Ok(_))
        );
    Json(json!({"database_healthy":database_healthy,
        "platform_restart_available":platform_target(&state).is_ok(),
        "message":if database_healthy { "平台数据库可用" } else { "数据库不可用；恢复操作仍可使用。重启平台不会自动修复 PostgreSQL，请管理员检查数据库服务与磁盘空间" }}))
}

async fn force_restart_platform(
    State(state): State<OperationsState>,
    uri: Uri,
    headers: HeaderMap,
    Json(credentials): Json<Credentials>,
) -> Result<Json<serde_json::Value>, OperationsError> {
    validate_origin(&headers, &uri)?;
    let target = platform_target(&state)?;
    force_restart(state, target.node_id, credentials, true).await
}

async fn force_restart_agent(
    State(state): State<OperationsState>,
    Path(node_id): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
    Json(credentials): Json<Credentials>,
) -> Result<Json<serde_json::Value>, OperationsError> {
    validate_origin(&headers, &uri)?;
    force_restart(state, node_id, credentials, false).await
}

async fn force_restart(
    state: OperationsState,
    node_id: Uuid,
    credentials: Credentials,
    platform: bool,
) -> Result<Json<serde_json::Value>, OperationsError> {
    validate_credentials(&credentials)?;
    let (target, lease) = target_and_lease(&state, node_id, true)?;
    if !target.restart_available() {
        return Err(OperationsError::bad_request("缺少可信 SSH 运维目标"));
    }
    let actor = credentials.username.clone();
    // Journal audit works when PostgreSQL is unavailable. Never log credentials.
    tracing::warn!(%node_id, %actor, platform, "force restart requested; administrator authentication required");
    let cancellation = Arc::new(Cancellation::default());
    let _guard = CancelOnDrop(cancellation.clone());
    let worker = tokio::task::spawn_blocking(move || {
        let _lease = lease;
        run_force_restart(&target, credentials, &cancellation, platform)
    });
    let result = match tokio::time::timeout(Duration::from_secs(65), worker).await {
        Ok(Ok(result)) => result,
        _ => Err(OperationsError::ssh(
            "强制重启超时；请核对服务日志和实际状态，避免重复提交",
        )),
    };
    tracing::warn!(%node_id, %actor, platform, success=result.is_ok(), "force restart result");
    result?;
    Ok(Json(json!({"status":"pending", "message":if platform {
        "平台强制重启已安排，将短暂断开连接；请稍后刷新。若数据库不可用，重启后进入恢复模式"
    } else { "Agent 服务已强制重启，已保留实际配置的权限；请以新心跳确认" }})))
}

fn run_force_restart(
    target: &Target,
    credentials: Credentials,
    cancellation: &Cancellation,
    platform: bool,
) -> Result<(), OperationsError> {
    let invocation = format!(
        "/usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin /usr/bin/timeout --kill-after=2s 45s /bin/bash --noprofile --norc -c {} -- {} {}",
        shell_quote(FORCE_RESTART_SCRIPT),
        if platform { "platform" } else { "agent" },
        shell_quote(&target.default_user)
    );
    run_privileged_command(target, credentials, cancellation, &invocation)
}

fn run_privileged_command(
    target: &Target,
    mut credentials: Credentials,
    cancellation: &Cancellation,
    invocation: &str,
) -> Result<(), OperationsError> {
    let session = ssh_connect(target, &credentials, cancellation)?;
    session.set_timeout(50_000);
    let command = if credentials.username == "root" {
        invocation.to_owned()
    } else {
        format!("/usr/bin/sudo -k -S -p '' {invocation}")
    };
    let mut channel = session
        .channel_session()
        .map_err(|_| OperationsError::ssh("无法打开管理员运维通道"))?;
    channel
        .handle_extended_data(ExtendedData::Ignore)
        .map_err(|_| OperationsError::ssh("无法初始化管理员运维通道"))?;
    channel
        .exec(&command)
        .map_err(|_| OperationsError::ssh("无法启动固定运维操作"))?;
    if credentials.username != "root" {
        channel
            .write_all(credentials.password.as_bytes())
            .and_then(|()| channel.write_all(b"\n"))
            .map_err(|_| OperationsError::ssh("无法验证管理员密码"))?;
    }
    credentials.password.zeroize();
    channel
        .send_eof()
        .map_err(|_| OperationsError::ssh("管理员认证通道中断"))?;
    let mut output = Vec::new();
    (&mut channel)
        .take(8193)
        .read_to_end(&mut output)
        .map_err(|_| OperationsError::ssh("运维连接中断；请核对实际服务状态"))?;
    channel
        .wait_close()
        .map_err(|_| OperationsError::ssh("运维通道未正常关闭"))?;
    if output.len() > 8192 || channel.exit_status().unwrap_or(-1) != 0 {
        return Err(OperationsError::ssh(
            "操作执行失败；请检查管理员 sudo 权限、候选包、服务配置及目标服务日志",
        ));
    }
    Ok(())
}

async fn list_targets(
    State(state): State<OperationsState>,
) -> Result<Json<serde_json::Value>, OperationsError> {
    let targets: Vec<_> = load_targets(&state.registry)?
        .into_iter()
        .map(|target| PublicTarget {
            elevation_available: target.elevation_available(),
            update_available: target.update_available(),
            update_state: target.update_state(),
            restart_available: target.restart_available(),
            node_id: target.node_id,
            hostname: target.hostname,
            host: target.host,
            port: target.port,
            default_user: target.default_user,
        })
        .collect();
    Ok(Json(json!({ "targets": targets })))
}

fn load_targets(path: &FsPath) -> Result<Vec<Target>, OperationsError> {
    Ok(load_all_targets(path)?
        .into_iter()
        .filter(|target| !target.disabled)
        .collect())
}

fn load_all_targets(path: &FsPath) -> Result<Vec<Target>, OperationsError> {
    let mut targets = read_target_file(path)?;
    for target in read_target_file(&path.with_file_name("managed-targets.json"))? {
        if let Some(previous) = targets
            .iter_mut()
            .find(|previous| previous.node_id == target.node_id)
        {
            // Existing fleet publication owns connection/digest fields. Browser
            // edits must not freeze an old candidate hash in the overlay.
            previous.hostname = target.hostname;
            previous.disabled = target.disabled;
            if target.connection_mode == ssh_transport::Mode::Ssh {
                previous.connection_mode = target.connection_mode;
                previous.ssh_remote_port = target.ssh_remote_port;
                previous.server_url = target.server_url;
                previous.agent_mode = target.agent_mode;
                previous.agent_sha256 = target.agent_sha256;
            }
        } else {
            targets.push(target);
        }
    }
    if targets.len() > 128 {
        return Err(OperationsError::registry());
    }
    Ok(targets)
}

pub(crate) fn health_targets(
    path: &FsPath,
) -> Result<Vec<(Uuid, String, IpAddr, u16, Option<bool>)>, &'static str> {
    load_targets(path)
        .map(|targets| {
            targets
                .into_iter()
                .map(|target| {
                    let connected = (target.connection_mode == ssh_transport::Mode::Ssh)
                        .then(|| ssh_transport::status(&target)["status"] == "connected");
                    (target.node_id, target.hostname, target.host, target.port, connected)
                })
                .collect()
        })
        .map_err(|_| "可信节点清单不可读取")
}

fn read_target_file(path: &FsPath) -> Result<Vec<Target>, OperationsError> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(OperationsError::registry()),
        Ok(_) => {}
    }
    check_private_file(path)?;
    let contents = std::fs::read(path).map_err(|_| OperationsError::registry())?;
    let registry: Registry =
        serde_json::from_slice(&contents).map_err(|_| OperationsError::registry())?;
    let targets = match registry {
        Registry::List(targets) | Registry::Wrapped { targets } => targets,
    };
    if targets.len() > 128 {
        return Err(OperationsError::registry());
    }
    let mut identifiers = HashSet::new();
    for target in &targets {
        if !identifiers.insert(target.node_id)
            || target.port == 0
            || target.ssh_remote_port == Some(0)
            || !valid_username(&target.default_user)
            || !valid_agent_home(&target.agent_home, &target.default_user)
            || !target.known_hosts.is_absolute()
            || target
                .agent_sha256
                .as_deref()
                .is_some_and(|hash| !valid_sha256(hash))
            || target
                .ca_sha256
                .as_deref()
                .is_some_and(|hash| !valid_sha256(hash))
            || target
                .server_url
                .as_deref()
                .is_some_and(|url| !valid_controller_url(url))
            || target
                .release_sha256
                .as_deref()
                .is_some_and(|hash| !valid_sha256(hash))
            || target
                .ssh_alias
                .as_deref()
                .is_some_and(|alias| !valid_ssh_alias(alias))
        {
            return Err(OperationsError::registry());
        }
    }
    Ok(targets)
}

fn valid_ssh_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= 128
        && !alias.starts_with('-')
        && alias
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.@-".contains(&byte))
}

fn release_root() -> Result<PathBuf, OperationsError> {
    if let Some(value) = std::env::var_os("EASYMANAGE_AGENT_RELEASE_ROOT") {
        let path = PathBuf::from(value);
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(OperationsError::registry());
        }
        return Ok(path);
    }
    let home = std::env::var_os("HOME").ok_or(OperationsError::registry())?;
    Ok(PathBuf::from(home).join(".easymanage/releases/current"))
}

async fn reported_agent_mode(pool: &PgPool, node_id: Uuid) -> Result<bool, OperationsError> {
    let row = sqlx::query("SELECT n.last_seen, m.metrics->'privilege'->>'mode' AS mode, m.metrics->'privilege'->>'uid' AS uid FROM nodes n LEFT JOIN metrics_latest m ON m.node_id=n.id WHERE n.id=$1")
        .bind(node_id).fetch_optional(pool).await.map_err(|_| OperationsError::busy())?
        .ok_or(OperationsError::bad_request("节点尚未注册"))?;
    let last_seen: chrono::DateTime<chrono::Utc> = row
        .try_get("last_seen")
        .map_err(|_| OperationsError::busy())?;
    if (chrono::Utc::now() - last_seen).num_seconds() > 45 {
        return Err(OperationsError::bad_request(
            "节点心跳已过期，暂不能安全操作",
        ));
    }
    let mode: Option<String> = row.try_get("mode").map_err(|_| OperationsError::busy())?;
    let uid: Option<String> = row.try_get("uid").map_err(|_| OperationsError::busy())?;
    match (
        mode.as_deref(),
        uid.and_then(|value| value.parse::<u32>().ok()),
    ) {
        (Some("full"), Some(0)) => Ok(true),
        (Some("basic"), Some(uid)) if uid != 0 => Ok(false),
        _ => Err(OperationsError::bad_request(
            "Agent 尚未上报可信的权限模式与 UID",
        )),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "update authorization, distribution, promotion and audit form one transaction"
)]
async fn update_agent(
    State(state): State<OperationsState>,
    Path(node_id): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
    Json(request): Json<UpdateRequest>,
) -> Result<Json<serde_json::Value>, OperationsError> {
    validate_origin(&headers, &uri)?;
    let (mut target, lease) = target_and_lease(&state, node_id, true)?;
    if !target.update_available() {
        return Err(OperationsError::bad_request(
            "先在主节点构建并发布 Agent 版本",
        ));
    }
    let root = release_root()?;
    let binary = root.join("target/release/easymanage-agent");
    let script = release_fleet_script(&root);
    check_release_file(&binary)?;
    if target.connection_mode != ssh_transport::Mode::Ssh { check_private_file(&script)?; }
    let release_hash = hex::encode(Sha256::digest(
        std::fs::read(&binary).map_err(|_| OperationsError::registry())?,
    ));
    if target.release_sha256.as_deref() != Some(release_hash.as_str()) {
        return Err(OperationsError::bad_request(
            "待分发版本的摘要与可信清单不一致",
        ));
    }
    let full = reported_agent_mode(&state.pool, node_id).await?;
    let credentials = if full {
        let username = request.username.ok_or(OperationsError::bad_request(
            "Full 更新需要目标机管理员用户名",
        ))?;
        let password = request.password.ok_or(OperationsError::bad_request(
            "Full 更新需要目标机管理员密码",
        ))?;
        let credentials = Credentials { username, password };
        validate_credentials(&credentials)?;
        Some(credentials)
    } else {
        None
    };
    let actor = credentials
        .as_ref()
        .map_or("ordinary-ssh-key".to_owned(), |value| {
            value.username.clone()
        });
    audit(
        &state.pool,
        node_id,
        &actor,
        "agent.update.request",
        "requested",
    )
    .await?;
    let cancellation = Arc::new(Cancellation::default());
    let _cancel_guard = CancelOnDrop(Arc::clone(&cancellation));
    let worker_state = state.clone();
    let worker = tokio::task::spawn_blocking(move || {
        let _lease = lease;
        let action = if full { "stage-full" } else { "deploy-agent" };
        if target.connection_mode == ssh_transport::Mode::Ssh {
            let stage = machines::update_over_ssh(&target, &binary, full)?;
            if full {
                machines::finish_full_ssh_update(&target, credentials.ok_or(OperationsError::registry())?, stage.as_deref().ok_or(OperationsError::registry())?, &release_hash, &cancellation)?;
            }
        } else {
            run_deployment(&script, &root, node_id, action)?;
            if full {
                target.agent_sha256 = Some(release_hash.clone());
                promote_agent(&target, credentials.ok_or(OperationsError::registry())?, &cancellation)?;
                run_deployment(&script, &root, node_id, "record-full")?;
            }
        }
        if target.connection_mode == ssh_transport::Mode::Ssh {
            target.agent_sha256 = Some(release_hash);
            machines::save_target(&worker_state, &target)?;
        }
        Ok::<_, OperationsError>(())
    });
    let result = match tokio::time::timeout(Duration::from_secs(125), worker).await {
        Ok(Ok(result)) => result,
        _ => Err(OperationsError::ssh(
            "Agent 分发超时；请检查目标机服务和部署记录",
        )),
    };
    if audit(
        &state.pool,
        node_id,
        &actor,
        "agent.update.result",
        if result.is_ok() { "success" } else { "failed" },
    )
    .await
    .is_err()
    {
        tracing::error!(%node_id, "unable to audit agent update result");
    }
    result?;
    Ok(Json(
        json!({ "status": "pending", "message": "Agent 已分发并启动；以随后上报的版本和心跳确认更新" }),
    ))
}

async fn restart_agent(
    State(state): State<OperationsState>,
    Path(node_id): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
    Json(request): Json<UpdateRequest>,
) -> Result<Json<serde_json::Value>, OperationsError> {
    validate_origin(&headers, &uri)?;
    let (target, lease) = target_and_lease(&state, node_id, true)?;
    if !target.restart_available() {
        return Err(OperationsError::bad_request("此节点缺少可信 SSH 运维目标"));
    }
    let full = reported_agent_mode(&state.pool, node_id).await?;
    let credentials = if full {
        let username = request.username.ok_or(OperationsError::bad_request(
            "Full Agent 重启需要管理员用户名",
        ))?;
        let password = request.password.ok_or(OperationsError::bad_request(
            "Full Agent 重启需要管理员密码",
        ))?;
        let credentials = Credentials { username, password };
        validate_credentials(&credentials)?;
        Some(credentials)
    } else {
        if request.username.is_some() || request.password.is_some() {
            return Err(OperationsError::bad_request(
                "Basic Agent 重启不需要管理员凭据",
            ));
        }
        None
    };
    let release = if full || target.connection_mode == ssh_transport::Mode::Ssh {
        None
    } else {
        let root = release_root()?;
        let script = release_fleet_script(&root);
        check_private_file(&script).map_err(|_| {
            OperationsError::bad_request("可信运维脚本尚未发布，请先发布 Agent 候选包")
        })?;
        Some((root, script))
    };
    let actor = credentials
        .as_ref()
        .map_or("ordinary-ssh-key".to_owned(), |value| {
            value.username.clone()
        });
    audit(
        &state.pool,
        node_id,
        &actor,
        "agent.restart.request",
        "requested",
    )
    .await?;
    let cancellation = Arc::new(Cancellation::default());
    let _cancel_guard = CancelOnDrop(Arc::clone(&cancellation));
    let worker = tokio::task::spawn_blocking(move || {
        let _lease = lease;
        if full {
            restart_full_agent(
                &target,
                credentials.ok_or(OperationsError::registry())?,
                &cancellation,
            )
        } else if target.connection_mode == ssh_transport::Mode::Ssh {
            let session = ssh_transport::primary(&target)?;
            session.set_timeout(50_000);
            machines::restart_basic_on(&session, &target)
        } else {
            let (root, script) = release.ok_or(OperationsError::registry())?;
            run_deployment(&script, &root, node_id, "restart-agent")
        }
    });
    let result = match tokio::time::timeout(Duration::from_secs(75), worker).await {
        Ok(Ok(result)) => result,
        _ => Err(OperationsError::ssh(
            "Agent 重启超时；请检查目标节点服务与新心跳",
        )),
    };
    if audit(
        &state.pool,
        node_id,
        &actor,
        "agent.restart.result",
        if result.is_ok() { "success" } else { "failed" },
    )
    .await
    .is_err()
    {
        tracing::error!(%node_id, "unable to audit agent restart result");
    }
    result?;
    Ok(Json(
        json!({ "status": "pending", "message": "Agent 服务已重启；请以随后收到的新心跳确认" }),
    ))
}

fn release_fleet_script(root: &FsPath) -> PathBuf {
    let current = root.join("deploy/orchestration/fleet.py");
    if current.is_file() {
        current
    } else {
        // Already-published private packages keep their original layout. The
        // caller applies the same ownership/permissions checks to either path.
        root.join("deploy/linux/fleet.py")
    }
}

fn run_deployment(
    script: &FsPath,
    release_root: &FsPath,
    node_id: Uuid,
    action: &str,
) -> Result<(), OperationsError> {
    let home = std::env::var("HOME").map_err(|_| OperationsError::registry())?;
    let user = std::env::var("USER").unwrap_or_default();
    let timeout = if action == "restart-agent" {
        "45s"
    } else {
        "100s"
    };
    let mut command = Command::new("/usr/bin/timeout");
    command
        .arg("--signal=TERM")
        .arg("--kill-after=5s")
        .arg(timeout)
        .arg("/usr/bin/python3")
        .arg(script)
        .arg(action)
        .arg("--node")
        .arg(node_id.to_string())
        .arg("--inventory")
        .arg({
            let private = release_root.join("deploy/local/fleet.json");
            // Older published releases stored their private inventory under this
            // historical name. New releases never publish the public example.
            if private.is_file() {
                private
            } else {
                release_root.join("deploy/fleet.example.json")
            }
        })
        .env_clear()
        .env("HOME", home)
        .env("USER", user)
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .current_dir(release_root);
    for name in [
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "EASYMANAGE_REMOTE_SSH_CONFIG",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let status = command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|_| OperationsError::ssh("无法执行 Agent 分发器"))?;
    if !status.success() {
        return Err(OperationsError::ssh(if action == "restart-agent" {
            "Basic Agent 重启未完成；请检查可信 SSH 和目标用户服务状态"
        } else {
            "Agent 分发未完成；请检查普通用户 SSH 和目标服务状态"
        }));
    }
    Ok(())
}

fn valid_sha256(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_controller_url(value: &str) -> bool {
    value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".:[]-/".contains(&byte))
        && reqwest::Url::parse(value).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/"
                && url.port_or_known_default().is_some_and(|port| port > 0)
        })
        && value.rsplit(':').next().is_some_and(|port| {
            let port = port.trim_end_matches('/');
            !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn check_private_file(path: &FsPath) -> Result<(), OperationsError> {
    check_bounded_private_file(path, 1024 * 1024)
}

fn check_release_file(path: &FsPath) -> Result<(), OperationsError> {
    check_bounded_private_file(path, 256 * 1024 * 1024)
}

fn check_bounded_private_file(path: &FsPath, limit: u64) -> Result<(), OperationsError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| OperationsError::registry())?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(OperationsError::registry());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o022 != 0 {
            return Err(OperationsError::registry());
        }
    }
    Ok(())
}

fn valid_username(username: &str) -> bool {
    !username.is_empty()
        && username.len() <= 64
        && !username.starts_with('-')
        && username
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-$".contains(&byte))
}

fn valid_agent_home(path: &FsPath, username: &str) -> bool {
    let expected = if username == "root" {
        PathBuf::from("/root/.easymanage")
    } else {
        PathBuf::from(format!("/home/{username}/.easymanage"))
    };
    path == expected
        && path
            .components()
            .all(|part| !matches!(part, Component::ParentDir))
}

fn validate_credentials(credentials: &Credentials) -> Result<(), OperationsError> {
    if !valid_username(&credentials.username)
        || credentials.password.is_empty()
        || credentials.password.len() > 1024
        || credentials
            .password
            .bytes()
            .any(|byte| b"\r\n\0".contains(&byte))
    {
        return Err(OperationsError::bad_request("用户名或密码格式不正确"));
    }
    Ok(())
}

pub(crate) fn validate_origin(headers: &HeaderMap, uri: &Uri) -> Result<(), OperationsError> {
    let host_header = headers.get("host").and_then(|value| value.to_str().ok());
    // HTTP/2 conveys the requested host in :authority, which hyper puts in Uri.
    // HTTP/1.1 normally has only Host. If both are present, they must agree.
    let authority = uri.authority().map(|value| value.as_str());
    let host = authority.or(host_header);
    let origin = headers.get("origin").and_then(|value| value.to_str().ok());
    if uri.scheme_str().is_some_and(|scheme| scheme != "https")
        || matches!((authority, host_header), (Some(authority), Some(header)) if !authority.eq_ignore_ascii_case(header))
        || !matches!((host, origin), (Some(host), Some(origin)) if origin == format!("https://{host}"))
        || headers
            .get("sec-fetch-site")
            .is_some_and(|value| value != "same-origin")
    {
        return Err(OperationsError(
            StatusCode::FORBIDDEN,
            "请从当前 HTTPS 平台页面发起操作：页面与接口必须使用相同的主机和端口（支持本机物理及虚拟网卡地址）",
        ));
    }
    Ok(())
}

fn target_and_lease(
    state: &OperationsState,
    node_id: Uuid,
    elevation: bool,
) -> Result<(Target, Lease), OperationsError> {
    let target = load_targets(&state.registry)?
        .into_iter()
        .find(|target| target.node_id == node_id)
        .ok_or(OperationsError(
            StatusCode::NOT_FOUND,
            "此节点尚未配置可信 SSH 运维目标",
        ))?;
    let mut limits = state.limits.lock().map_err(|_| OperationsError::busy())?;
    let now = Instant::now();
    while limits
        .attempts
        .front()
        .is_some_and(|(at, _)| now.duration_since(*at) > Duration::from_secs(60))
    {
        limits.attempts.pop_front();
    }
    if limits.active.values().sum::<usize>() >= 8
        || (elevation && limits.elevating.contains(&node_id))
        || limits.active.get(&node_id).copied().unwrap_or_default() >= 2
        || limits.attempts.len() >= 30
        || limits
            .attempts
            .iter()
            .filter(|(_, id)| *id == node_id)
            .count()
            >= 6
    {
        return Err(OperationsError::busy());
    }
    limits.attempts.push_back((now, node_id));
    *limits.active.entry(node_id).or_default() += 1;
    if elevation {
        limits.elevating.insert(node_id);
    }
    Ok((
        target,
        Lease {
            limits: Arc::clone(&state.limits),
            node_id,
            elevation,
        },
    ))
}

async fn audit(
    pool: &PgPool,
    node_id: Uuid,
    username: &str,
    action: &str,
    result: &str,
) -> Result<(), OperationsError> {
    sqlx::query(
        "INSERT INTO audit_logs (actor_type, actor_id, action, target_type, target_id, details) \
         VALUES ('ssh-user', $1, $2, 'node', $3, $4)",
    )
    .bind(username)
    .bind(action)
    .bind(node_id.to_string())
    .bind(json!({ "result": result }))
    .execute(pool)
    .await
    .map_err(|_| {
        OperationsError(
            StatusCode::SERVICE_UNAVAILABLE,
            "审计记录暂时不可用，操作未获准继续",
        )
    })?;
    Ok(())
}

#[derive(Default)]
struct Cancellation {
    cancelled: AtomicBool,
    socket: Mutex<Option<TcpStream>>,
}

impl Cancellation {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Ok(socket) = self.socket.lock()
            && let Some(socket) = socket.as_ref()
        {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

struct CancelOnDrop(Arc<Cancellation>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

fn ssh_connect(
    target: &Target,
    credentials: &Credentials,
    cancellation: &Cancellation,
) -> Result<ssh_transport::SessionLease, OperationsError> {
    ssh_transport::operation(target, credentials, cancellation)
}

async fn elevate(
    State(state): State<OperationsState>,
    Path(node_id): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
    Json(credentials): Json<Credentials>,
) -> Result<Json<serde_json::Value>, OperationsError> {
    validate_origin(&headers, &uri)?;
    validate_credentials(&credentials)?;
    let (target, lease) = target_and_lease(&state, node_id, true)?;
    if !target.elevation_available() {
        return Err(OperationsError::bad_request(
            "此节点缺少可信 Agent、CA 或控制端配置，请先通过主节点重新下发",
        ));
    }
    let username = credentials.username.clone();
    audit(
        &state.pool,
        node_id,
        &username,
        "agent.elevate.request",
        "requested",
    )
    .await?;
    let cancellation = Arc::new(Cancellation::default());
    let _cancel_guard = CancelOnDrop(Arc::clone(&cancellation));
    let worker = tokio::task::spawn_blocking(move || {
        let _lease = lease;
        promote_agent(&target, credentials, &cancellation)
    });
    let result = match tokio::time::timeout(ELEVATION_TIMEOUT + SSH_TIMEOUT * 3, worker).await {
        Ok(Ok(result)) => result,
        _ => Err(OperationsError::ssh(
            "提权操作超时或连接已中断，请检查目标机 Agent 服务状态",
        )),
    };
    if audit(
        &state.pool,
        node_id,
        &username,
        "agent.elevate.result",
        if result.is_ok() { "success" } else { "failed" },
    )
    .await
    .is_err()
    {
        tracing::error!(%node_id, "unable to record agent elevation result");
    }
    result?;
    Ok(Json(
        json!({ "status": "pending", "message": "管理员身份已验证，Full Agent 服务已启动；等待新心跳确认权限和指标" }),
    ))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

async fn manage_user(
    State(state): State<OperationsState>,
    Path(node_id): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
    Json(request): Json<UserActionRequest>,
) -> Result<Json<serde_json::Value>, OperationsError> {
    validate_origin(&headers, &uri)?;
    if !matches!(
        request.action.as_str(),
        "list" | "create" | "update" | "delete"
    ) || (request.action != "list" && !valid_managed_username(&request.subject))
        || request.display_name.len() > 100
        || request
            .display_name
            .bytes()
            .any(|byte| b":\r\n\0".contains(&byte))
        || !matches!(
            request.shell.as_str(),
            "" | "/bin/bash" | "/bin/zsh" | "/usr/bin/zsh" | "/usr/sbin/nologin"
        )
        || request.groups.as_ref().is_some_and(|groups| {
            groups.len() > 16 || groups.iter().any(|name| !valid_managed_username(name))
        })
        || (request.action != "list" && request.confirmation != request.subject)
    {
        return Err(OperationsError::bad_request(
            "账户操作参数无效或确认名称不一致",
        ));
    }
    let UserActionRequest {
        username,
        password,
        action,
        subject,
        display_name,
        shell,
        groups,
        confirmation,
    } = request;
    let operation = UserOperation {
        action,
        subject,
        display_name,
        shell,
        groups,
        confirmation,
    };
    let credentials = Credentials { username, password };
    validate_credentials(&credentials)?;
    if !valid_managed_username(&credentials.username) {
        return Err(OperationsError::bad_request(
            "管理员账户名称不符合本地账户格式",
        ));
    }
    let (target, lease) = target_and_lease(&state, node_id, true)?;
    let action = operation.action.clone();
    audit(
        &state.pool,
        node_id,
        &credentials.username,
        &format!("user.{action}.request"),
        "requested",
    )
    .await?;
    let actor = credentials.username.clone();
    let cancellation = Arc::new(Cancellation::default());
    let _cancel_guard = CancelOnDrop(Arc::clone(&cancellation));
    let worker = tokio::task::spawn_blocking(move || {
        let _lease = lease;
        manage_user_worker(&target, credentials, operation, &cancellation)
    });
    let result = match tokio::time::timeout(Duration::from_secs(50), worker).await {
        Ok(Ok(result)) => result,
        _ => Err(OperationsError::ssh(
            "账户操作超时或连接中断，请检查目标主机状态",
        )),
    };
    if audit(
        &state.pool,
        node_id,
        &actor,
        &format!("user.{action}.result"),
        if result.is_ok() { "success" } else { "failed" },
    )
    .await
    .is_err()
    {
        tracing::error!(%node_id, "unable to record user-operation result");
    }
    Ok(Json(result?))
}

fn valid_managed_username(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && !value.starts_with('-')
        && value.bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte.is_ascii_lowercase() || byte == b'_'
            } else {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte)
            }
        })
}

fn manage_user_worker(
    target: &Target,
    mut credentials: Credentials,
    request: UserOperation,
    cancellation: &Cancellation,
) -> Result<serde_json::Value, OperationsError> {
    let session = ssh_connect(target, &credentials, cancellation)?;
    session.set_timeout(35_000);
    let groups = request.groups.map_or_else(
        || {
            if request.action == "update" {
                "__keep__".to_owned()
            } else {
                String::new()
            }
        },
        |groups| groups.join(","),
    );
    let args = [
        request.action.as_str(),
        credentials.username.as_str(),
        if request.action == "list" {
            "root"
        } else {
            &request.subject
        },
        &request.display_name,
        &request.shell,
        &groups,
        &request.confirmation,
    ];
    let quoted = args
        .iter()
        .map(|value| shell_quote(value))
        .collect::<Vec<_>>()
        .join(" ");
    let invocation = format!(
        "/usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin /usr/bin/timeout --signal=TERM --kill-after=1s 30s /bin/bash --noprofile --norc -c {} -- {quoted}",
        shell_quote(USER_SCRIPT),
    );
    let command = if credentials.username == "root" {
        invocation
    } else {
        format!("/usr/bin/sudo -k -S -p '' {invocation}")
    };
    let mut channel = session
        .channel_session()
        .map_err(|_| OperationsError::ssh("无法打开账户管理通道"))?;
    channel
        .handle_extended_data(ExtendedData::Ignore)
        .map_err(|_| OperationsError::ssh("无法初始化账户管理通道"))?;
    channel
        .exec(&command)
        .map_err(|_| OperationsError::ssh("无法启动账户管理操作"))?;
    if credentials.username != "root" {
        channel
            .write_all(credentials.password.as_bytes())
            .map_err(|_| OperationsError::ssh("无法验证管理员密码"))?;
        channel
            .write_all(b"\n")
            .map_err(|_| OperationsError::ssh("无法验证管理员密码"))?;
    }
    credentials.password.zeroize();
    channel
        .send_eof()
        .map_err(|_| OperationsError::ssh("管理员认证通道中断"))?;
    let mut output = Vec::new();
    (&mut channel)
        .take(128 * 1024 + 1)
        .read_to_end(&mut output)
        .map_err(|_| OperationsError::ssh("账户管理操作中断"))?;
    if output.len() > 128 * 1024 {
        return Err(OperationsError::ssh("账户清单超过安全上限"));
    }
    channel
        .wait_close()
        .map_err(|_| OperationsError::ssh("账户管理通道未正常关闭"))?;
    if channel.exit_status().unwrap_or(-1) != 0 {
        return Err(OperationsError::ssh(
            "账户操作未完成；请核对管理员 sudo 权限、目标账户和当前登录进程",
        ));
    }
    if request.action == "list" {
        serde_json::from_slice(&output)
            .map_err(|_| OperationsError::ssh("目标主机返回无效账户清单"))
    } else {
        Ok(
            json!({ "status": "ok", "message": "目标主机已执行账户操作；保留审计记录。删除账户时用户目录及文件仍保留。" }),
        )
    }
}

fn promote_agent(
    target: &Target,
    mut credentials: Credentials,
    cancellation: &Cancellation,
) -> Result<(), OperationsError> {
    let session = ssh_connect(target, &credentials, cancellation)?;
    let agent_home = target
        .agent_home
        .to_str()
        .ok_or(OperationsError::registry())?;
    let hash = target
        .agent_sha256
        .as_deref()
        .ok_or(OperationsError::registry())?;
    let server_url = target
        .server_url
        .as_deref()
        .ok_or(OperationsError::registry())?;
    let ca_hash = target
        .ca_sha256
        .as_deref()
        .ok_or(OperationsError::registry())?;
    let invocation = format!(
        "/usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin /bin/bash --noprofile --norc -c {} -- {} {} {} {} {} {}",
        shell_quote(PROMOTION_SCRIPT),
        shell_quote(agent_home),
        shell_quote(&target.default_user),
        shell_quote(hash),
        shell_quote(&target.node_id.to_string()),
        shell_quote(server_url),
        shell_quote(ca_hash),
    );
    let command = if credentials.username == "root" {
        invocation
    } else {
        format!("/usr/bin/sudo -k -S -p '' {invocation}")
    };
    let mut channel = session
        .channel_session()
        .map_err(|_| OperationsError::ssh("无法打开 SSH 提权通道"))?;
    channel
        .handle_extended_data(ExtendedData::Merge)
        .map_err(|_| OperationsError::ssh("无法初始化提权通道"))?;
    channel
        .exec(&command)
        .map_err(|_| OperationsError::ssh("无法启动固定 Agent 提权安装操作"))?;
    if credentials.username != "root" {
        channel
            .write_all(credentials.password.as_bytes())
            .map_err(|_| OperationsError::ssh("无法提交 sudo 认证"))?;
        channel
            .write_all(b"\n")
            .map_err(|_| OperationsError::ssh("无法提交 sudo 认证"))?;
    }
    credentials.password.zeroize();
    channel
        .send_eof()
        .map_err(|_| OperationsError::ssh("提权通道已中断"))?;
    session.set_blocking(false);
    let started = Instant::now();
    let mut output = Zeroizing::new([0_u8; 4096]);
    let mut total_output = 0_usize;
    loop {
        if cancellation.is_cancelled() || started.elapsed() > ELEVATION_TIMEOUT {
            return Err(OperationsError::ssh(
                "提权超时或已取消，请检查目标机 Agent 服务状态",
            ));
        }
        match channel.read(&mut *output) {
            Ok(size) => total_output += size,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => {
                return Err(OperationsError::ssh(
                    "提权连接中断，请检查目标机 Agent 服务状态",
                ));
            }
        }
        if total_output > 64 * 1024 {
            return Err(OperationsError::ssh("提权操作输出超过限制"));
        }
        if channel.eof() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    session.set_blocking(true);
    session.set_timeout(2_000);
    let _ = channel.wait_close();
    if channel.exit_status().unwrap_or(-1) != 0 {
        return Err(OperationsError::ssh(
            "提权失败：请确认该账户具有 sudo 权限、Agent 安装文件完整，并查看目标机服务日志",
        ));
    }
    Ok(())
}

fn restart_full_agent(
    target: &Target,
    mut credentials: Credentials,
    cancellation: &Cancellation,
) -> Result<(), OperationsError> {
    let session = ssh_connect(target, &credentials, cancellation)?;
    session.set_timeout(40_000);
    let invocation = format!(
        "/usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin /usr/bin/timeout --signal=TERM --kill-after=2s 30s /bin/bash --noprofile --norc -c {}",
        shell_quote(FULL_RESTART_SCRIPT),
    );
    let command = if credentials.username == "root" {
        invocation
    } else {
        format!("/usr/bin/sudo -k -S -p '' {invocation}")
    };
    let mut channel = session
        .channel_session()
        .map_err(|_| OperationsError::ssh("无法打开 Agent 重启通道"))?;
    channel
        .handle_extended_data(ExtendedData::Ignore)
        .map_err(|_| OperationsError::ssh("无法初始化 Agent 重启通道"))?;
    channel
        .exec(&command)
        .map_err(|_| OperationsError::ssh("无法启动固定 Agent 重启操作"))?;
    if credentials.username != "root" {
        channel
            .write_all(credentials.password.as_bytes())
            .map_err(|_| OperationsError::ssh("无法验证管理员密码"))?;
        channel
            .write_all(b"\n")
            .map_err(|_| OperationsError::ssh("无法验证管理员密码"))?;
    }
    credentials.password.zeroize();
    channel
        .send_eof()
        .map_err(|_| OperationsError::ssh("管理员认证通道中断"))?;
    let mut output = Vec::new();
    (&mut channel)
        .take(4 * 1024 + 1)
        .read_to_end(&mut output)
        .map_err(|_| OperationsError::ssh("Agent 重启连接中断"))?;
    if output.len() > 4 * 1024 {
        return Err(OperationsError::ssh("Agent 重启输出超过安全上限"));
    }
    channel
        .wait_close()
        .map_err(|_| OperationsError::ssh("Agent 重启通道未正常关闭"))?;
    if channel.exit_status().unwrap_or(-1) != 0 {
        return Err(OperationsError::ssh(
            "Full Agent 重启失败；请检查管理员 sudo 权限及目标服务状态",
        ));
    }
    Ok(())
}

async fn terminal(
    State(state): State<OperationsState>,
    Path(node_id): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, OperationsError> {
    validate_origin(&headers, &uri)?;
    let (target, lease) = target_and_lease(&state, node_id, false)?;
    Ok(upgrade
        .max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| terminal_socket(socket, state, target, lease)))
}

async fn send_event(socket: &mut WebSocket, event: TerminalEvent) -> bool {
    let message = match event {
        TerminalEvent::Ready => json!({ "type": "ready" }),
        TerminalEvent::Output(data) => json!({ "type": "output", "data": data }),
        TerminalEvent::Error(message) => json!({ "type": "error", "message": message }),
        TerminalEvent::Closed => json!({ "type": "closed" }),
    };
    matches!(
        tokio::time::timeout(
            Duration::from_secs(5),
            socket.send(Message::Text(message.to_string().into()))
        )
        .await,
        Ok(Ok(()))
    )
}

async fn receive_terminal_credentials(
    socket: &mut WebSocket,
    state: &OperationsState,
    node_id: Uuid,
) -> Result<(Credentials, (u32, u32)), OperationsError> {
    let authentication = match tokio::time::timeout(AUTH_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(frame)))) => serde_json::from_str::<BrowserMessage>(&frame).ok(),
        _ => None,
    };
    let Some(BrowserMessage::Auth {
        username,
        password,
        cols,
        rows,
    }) = authentication
    else {
        return Err(OperationsError::bad_request(
            "请在连接后 15 秒内提交 SSH 用户名和密码",
        ));
    };
    let credentials = Credentials { username, password };
    validate_credentials(&credentials)?;
    audit(
        &state.pool,
        node_id,
        &credentials.username,
        "terminal.request",
        "requested",
    )
    .await?;
    Ok((credentials, (cols, rows)))
}

async fn terminal_socket(
    mut socket: WebSocket,
    state: OperationsState,
    target: Target,
    lease: Lease,
) {
    let node_id = target.node_id;
    let (credentials, dimensions) =
        match receive_terminal_credentials(&mut socket, &state, node_id).await {
            Ok(authentication) => authentication,
            Err(error) => {
                let _ = send_event(&mut socket, TerminalEvent::Error(error.1)).await;
                return;
            }
        };
    let username = credentials.username.clone();
    let cancellation = Arc::new(Cancellation::default());
    let _cancel_guard = CancelOnDrop(Arc::clone(&cancellation));
    let (input_sender, input_receiver) = mpsc::channel(16);
    let (event_sender, mut event_receiver) = mpsc::channel(16);
    let worker_cancellation = Arc::clone(&cancellation);
    let worker = tokio::task::spawn_blocking(move || {
        let _lease = lease;
        let result = terminal_worker(
            &target,
            credentials,
            dimensions,
            input_receiver,
            &event_sender,
            &worker_cancellation,
        );
        if let Err(error) = result {
            let _ = event_sender.try_send(TerminalEvent::Error(error.1));
        }
        let _ = event_sender.try_send(TerminalEvent::Closed);
    });
    let started = Instant::now();
    let mut last_input = Instant::now();
    let mut timer = tokio::time::interval(Duration::from_secs(20));
    loop {
        tokio::select! {
            frame = socket.recv() => {
                match frame {
                    Some(Ok(Message::Text(frame))) => {
                        let message = serde_json::from_str::<BrowserMessage>(&frame);
                        let input = match message {
                            Ok(BrowserMessage::Input { data }) if data.len() <= 8192 => TerminalInput::Data(Zeroizing::new(data.into_bytes())),
                            Ok(BrowserMessage::Resize { cols, rows }) => TerminalInput::Resize(cols.clamp(20, 400), rows.clamp(5, 150)),
                            _ => {
                                let _ = send_event(&mut socket, TerminalEvent::Error("无效的终端消息")).await;
                                break;
                            }
                        };
                        last_input = Instant::now();
                        if input_sender.try_send(input).is_err() {
                            let _ = send_event(&mut socket, TerminalEvent::Error("终端输入过快或连接已关闭")).await;
                            break;
                        }
                    },
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {},
                    _ => break,
                }
            },
            event = event_receiver.recv() => {
                let Some(event) = event else { break; };
                let closed = matches!(event, TerminalEvent::Closed | TerminalEvent::Error(_));
                if matches!(event, TerminalEvent::Ready)
                    && audit(&state.pool, node_id, &username, "terminal.open", "authenticated").await.is_err()
                {
                    let _ = send_event(&mut socket, TerminalEvent::Error("审计记录暂时不可用，终端已关闭")).await;
                    break;
                }
                if matches!(event, TerminalEvent::Error(_))
                    && audit(&state.pool, node_id, &username, "terminal.error", "failed").await.is_err()
                {
                    tracing::error!(%node_id, "unable to record terminal failure");
                }
                if !send_event(&mut socket, event).await || closed { break; }
            },
            _ = timer.tick() => {
                if last_input.elapsed() > IDLE_TIMEOUT || started.elapsed() > MAX_SESSION_TIME {
                    let _ = send_event(&mut socket, TerminalEvent::Error("终端已达到空闲或会话时限，请重新登录")).await;
                    break;
                }
                if !matches!(tokio::time::timeout(Duration::from_secs(5), socket.send(Message::Ping(Vec::new().into()))).await, Ok(Ok(()))) { break; }
            },
        }
    }
    cancellation.cancel();
    drop(input_sender);
    let _ = tokio::time::timeout(Duration::from_secs(12), worker).await;
    if audit(&state.pool, node_id, &username, "terminal.close", "closed")
        .await
        .is_err()
    {
        tracing::error!(%node_id, "unable to record terminal close");
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), socket.send(Message::Close(None))).await;
}

fn terminal_worker(
    target: &Target,
    mut credentials: Credentials,
    dimensions: (u32, u32),
    mut input: mpsc::Receiver<TerminalInput>,
    events: &mpsc::Sender<TerminalEvent>,
    cancellation: &Cancellation,
) -> Result<(), OperationsError> {
    let session = ssh_connect(target, &credentials, cancellation)?;
    credentials.password.zeroize();
    let mut channel = session
        .channel_session()
        .map_err(|_| OperationsError::ssh("无法打开 SSH 终端"))?;
    channel
        .request_pty(
            "xterm-256color",
            None,
            Some((
                dimensions.0.clamp(20, 400),
                dimensions.1.clamp(5, 150),
                0,
                0,
            )),
        )
        .map_err(|_| OperationsError::ssh("目标账户不允许分配终端"))?;
    channel
        .handle_extended_data(ExtendedData::Merge)
        .map_err(|_| OperationsError::ssh("无法初始化终端"))?;
    channel
        .exec("/bin/bash -l")
        .map_err(|_| OperationsError::ssh("目标账户无法启动 bash"))?;
    session.set_blocking(false);
    events
        .try_send(TerminalEvent::Ready)
        .map_err(|_| OperationsError::ssh("浏览器连接已关闭"))?;
    let mut pending = Zeroizing::new(Vec::<u8>::new());
    let mut pending_offset = 0;
    let mut resize = None;
    let mut buffer = Zeroizing::new([0_u8; 8192]);
    let mut decoder = OutputDecoder::default();
    let started = Instant::now();
    let mut last_input = Instant::now();
    loop {
        if cancellation.is_cancelled()
            || started.elapsed() > MAX_SESSION_TIME
            || last_input.elapsed() > IDLE_TIMEOUT
        {
            break;
        }
        if pending.is_empty() {
            match input.try_recv() {
                Ok(TerminalInput::Data(data)) => {
                    pending = data;
                    last_input = Instant::now();
                }
                Ok(TerminalInput::Resize(cols, rows)) => {
                    resize = Some((cols, rows));
                    last_input = Instant::now();
                }
                Err(mpsc::error::TryRecvError::Disconnected) => break,
                Err(mpsc::error::TryRecvError::Empty) => {}
            }
        }
        if !pending.is_empty() {
            match channel.write(&pending[pending_offset..]) {
                Ok(0) => break,
                Ok(size) => {
                    pending_offset += size;
                    if pending_offset == pending.len() {
                        pending.zeroize();
                        pending_offset = 0;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Err(OperationsError::ssh("终端输入连接已中断")),
            }
        }
        if let Some((cols, rows)) = resize {
            match channel.request_pty_size(cols, rows, None, None) {
                Ok(()) => resize = None,
                Err(error) if error.code() == ssh2::ErrorCode::Session(-37) => {}
                Err(_) => return Err(OperationsError::ssh("终端窗口调整失败")),
            }
        }
        match channel.read(&mut *buffer) {
            Ok(size) if size > 0 => {
                let output = decoder.push(&buffer[..size]);
                if !output.is_empty() && events.try_send(TerminalEvent::Output(output)).is_err() {
                    return Err(OperationsError::ssh("浏览器接收过慢，终端已关闭"));
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return Err(OperationsError::ssh("终端输出连接已中断")),
        }
        if channel.eof() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Closing the SSH transport releases the remote PTY and its foreground shell.
    cancellation.cancel();
    Ok(())
}

#[derive(Default)]
struct OutputDecoder {
    incomplete: Vec<u8>,
}

impl OutputDecoder {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.incomplete.extend_from_slice(bytes);
        let mut output = String::new();
        loop {
            match std::str::from_utf8(&self.incomplete) {
                Ok(text) => {
                    output.push_str(text);
                    self.incomplete.clear();
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    output.push_str(
                        std::str::from_utf8(&self.incomplete[..valid]).unwrap_or_default(),
                    );
                    self.incomplete.drain(..valid);
                    if let Some(length) = error.error_len() {
                        output.push('\u{fffd}');
                        self.incomplete.drain(..length);
                    } else {
                        break;
                    }
                }
            }
        }
        output
    }
}

pub(crate) struct OperationsError(StatusCode, &'static str);

impl OperationsError {
    const fn bad_request(message: &'static str) -> Self {
        Self(StatusCode::BAD_REQUEST, message)
    }

    const fn registry() -> Self {
        Self(
            StatusCode::SERVICE_UNAVAILABLE,
            "可信运维配置不可用：检查 targets.json 和 known_hosts 的内容及文件权限",
        )
    }

    const fn ssh(message: &'static str) -> Self {
        Self(StatusCode::BAD_GATEWAY, message)
    }

    const fn busy() -> Self {
        Self(
            StatusCode::TOO_MANY_REQUESTS,
            "运维连接过多或尝试过于频繁，请稍后重试",
        )
    }
}

impl IntoResponse for OperationsError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1, "message": self.1 }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn browser_overlay_preserves_published_digests_and_can_disable_a_target() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("easymanage-overlay-test-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let id = Uuid::new_v4();
        let base = json!({"node_id":id,"hostname":"base","host":"10.0.0.1","port":22,
            "default_user":"tester","agent_home":"/home/tester/.easymanage",
            "known_hosts":"/tmp/fixture-known-hosts","agent_sha256":"a".repeat(64),"release_sha256":"b".repeat(64)});
        let path = root.join("targets.json");
        let overlay = root.join("managed-targets.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({"targets":[base.clone()]})).unwrap(),
        )
        .unwrap();
        let mut override_target = base;
        override_target["hostname"] = json!("renamed");
        override_target["disabled"] = json!(true);
        override_target["release_sha256"] = json!("c".repeat(64));
        std::fs::write(
            &overlay,
            serde_json::to_vec(&json!({"targets":[override_target]})).unwrap(),
        )
        .unwrap();
        for file in [&path, &overlay] {
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(
            load_targets(&path)
                .unwrap_or_else(|error| panic!("{}", error.1))
                .is_empty()
        );
        let all = load_all_targets(&path).unwrap_or_else(|error| panic!("{}", error.1));
        assert_eq!(all[0].hostname, "renamed");
        assert_eq!(
            all[0].release_sha256.as_deref(),
            Some("b".repeat(64).as_str())
        );
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(overlay).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn published_fleet_layout_prefers_current_and_preserves_old_releases() {
        let root =
            std::env::temp_dir().join(format!("easymanage-release-layout-{}", Uuid::new_v4()));
        let old = root.join("deploy/linux/fleet.py");
        let current = root.join("deploy/orchestration/fleet.py");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, b"isolated old release fixture").unwrap();
        assert_eq!(release_fleet_script(&root), old);
        std::fs::create_dir_all(current.parent().unwrap()).unwrap();
        std::fs::write(&current, b"isolated current release fixture").unwrap();
        assert_eq!(release_fleet_script(&root), current);
        std::fs::remove_file(&old).unwrap();
        std::fs::remove_file(&current).unwrap();
        std::fs::remove_dir(root.join("deploy/linux")).unwrap();
        std::fs::remove_dir(root.join("deploy/orchestration")).unwrap();
        std::fs::remove_dir(root.join("deploy")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn candidate_update_requires_a_different_known_installed_digest() {
        let mut target = Target {
            node_id: Uuid::nil(),
            hostname: "fixture".to_owned(),
            host: "127.0.0.1".parse().unwrap(),
            port: 22,
            default_user: "tester".to_owned(),
            agent_home: PathBuf::from("/home/tester/.easymanage"),
            known_hosts: PathBuf::from("/tmp/fixture-known-hosts"),
            agent_sha256: Some("a".repeat(64)),
            server_url: None,
            ca_sha256: None,
            release_sha256: Some("A".repeat(64)),
            ssh_alias: Some("fixture".to_owned()),
            disabled: false,
            connection_mode: ssh_transport::Mode::Direct,
            ssh_remote_port: None,
            agent_mode: None,
        };
        assert_eq!(target.update_state(), "current");
        assert!(!target.update_available());
        target.release_sha256 = Some("b".repeat(64));
        assert_eq!(target.update_state(), "available");
        assert!(target.update_available());
        target.agent_sha256 = None;
        assert_eq!(target.update_state(), "unknown_installed");
        assert!(!target.update_available());
        target.release_sha256 = None;
        assert_eq!(target.update_state(), "not_published");
    }

    #[test]
    fn operations_require_exact_https_origin_and_port() {
        let mut headers = HeaderMap::new();
        let http1_uri: Uri = "/api/v1/nodes/123/elevate".parse().unwrap();
        for host in [
            "192.168.10.110:8443",      // physical LAN
            "100.108.29.94:8443",       // Tailscale
            "10.78.11.136:8443",        // ZeroTier
            "[fd7a:115c:a1e0::1]:8443", // IPv6 overlay
        ] {
            headers.insert("host", host.parse().unwrap());
            headers.insert("origin", format!("https://{host}").parse().unwrap());
            assert!(
                validate_origin(&headers, &http1_uri).is_ok(),
                "HTTP/1.1 on {host}"
            );
            let http2_uri: Uri = format!("https://{host}/api/v1/nodes/123/elevate")
                .parse()
                .unwrap();
            headers.remove("host");
            assert!(
                validate_origin(&headers, &http2_uri).is_ok(),
                "HTTP/2 on {host}"
            );
            headers.insert("host", host.parse().unwrap());
            assert!(
                validate_origin(&headers, &http2_uri).is_ok(),
                "HTTP/2 with Host on {host}"
            );
        }
        headers.insert("host", "10.78.11.136:8443".parse().unwrap());
        for origin in [
            "null",
            "http://10.78.11.136:8443",
            "https://10.78.11.136",
            "https://attacker.example",
            "https://10.78.11.136:8443.attacker.example",
        ] {
            headers.insert("origin", origin.parse().unwrap());
            assert!(validate_origin(&headers, &http1_uri).is_err());
        }
        headers.insert("origin", "https://10.78.11.136:8443".parse().unwrap());
        assert!(validate_origin(&headers, &http1_uri).is_ok());
        headers.insert("origin", "https://100.108.29.94:8443".parse().unwrap());
        assert!(validate_origin(&headers, &http1_uri).is_err());
        headers.insert("origin", "https://10.78.11.136:8443".parse().unwrap());
        let http2_uri: Uri = "https://10.78.11.136:8443/api/v1/nodes/123/elevate"
            .parse()
            .unwrap();
        headers.insert("host", "100.108.29.94:8443".parse().unwrap());
        assert!(validate_origin(&headers, &http2_uri).is_err());
        headers.remove("host");
        let insecure_uri: Uri = "http://10.78.11.136:8443/api/v1/nodes/123/elevate"
            .parse()
            .unwrap();
        assert!(validate_origin(&headers, &insecure_uri).is_err());
        headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert!(validate_origin(&headers, &http2_uri).is_err());
        headers.remove("origin");
        assert!(validate_origin(&headers, &http2_uri).is_err());
    }

    #[test]
    fn identity_and_deployment_paths_reject_shell_and_traversal_input() {
        for username in ["-root", "root;id", "$(id)", "../root", "root\n", ""] {
            assert!(!valid_username(username));
        }
        assert!(valid_agent_home(
            FsPath::new("/home/tzr/.easymanage"),
            "tzr"
        ));
        for path in [
            "/home/tzr/../root/.easymanage",
            "/home/other/.easymanage",
            "/tmp/easymanage",
            "/home/tzr/.easymanage/child",
        ] {
            assert!(!valid_agent_home(FsPath::new(path), "tzr"));
        }
        for name in ["-a", "root;id", "../user", "user name", "User", "a\n", ""] {
            assert!(!valid_managed_username(name));
        }
        assert!(valid_managed_username("lab_user-2"));
        for alias in ["-oProxyCommand=touch", "node;id", "../node", "$(id)"] {
            assert!(!valid_ssh_alias(alias));
        }
        assert!(valid_ssh_alias("1110-server2-tzr"));
    }

    #[test]
    fn terminal_preserves_utf8_across_arbitrary_ssh_chunks() {
        let text = "操作终端 🌏\r\nuser@server:~$ ";
        for chunk_size in 1..=text.len() {
            let mut decoder = OutputDecoder::default();
            let reconstructed: String = text
                .as_bytes()
                .chunks(chunk_size)
                .map(|chunk| decoder.push(chunk))
                .collect();
            assert_eq!(reconstructed, text);
            assert!(decoder.incomplete.is_empty());
        }
    }

    #[test]
    fn terminal_replaces_invalid_bytes_without_losing_valid_suffix() {
        let mut decoder = OutputDecoder::default();
        assert_eq!(decoder.push(b"ok\xff\xe4"), "ok\u{fffd}");
        assert_eq!(decoder.push(&[0xb8, 0xad]), "中");
    }
}
