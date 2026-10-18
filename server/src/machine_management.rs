//! Password/public-key onboarding. Secrets live only in transient request/worker
//! memory; job logs contain fixed workflow messages, never raw SSH output.
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
use chrono::{DateTime, Utc};
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use tokio::sync::Semaphore;

const USER_INSTALL: &str = include_str!("../../deploy/linux/user/install-user-agent.sh");
const USER_UNIT: &str = include_str!("../../deploy/linux/user/easymanage-agent.user.service");
const USER_CONTROL: &str = include_str!("../../deploy/linux/user/agent-user-control.py");
const FULL_INSTALL: &str = include_str!("../../deploy/linux/operations/install-new-full-agent.sh");
const JOB_TTL: Duration = Duration::from_mins(30);
const MAX_PRIVATE_KEY_BYTES: usize = 12 * 1024;

#[derive(Clone)]
pub(super) struct Tasks {
    jobs: Arc<Mutex<HashMap<Uuid, Job>>>,
    writes: Arc<Mutex<()>>,
    attempts: Arc<Mutex<VecDeque<Instant>>>,
    permits: Arc<Semaphore>,
}

impl Default for Tasks {
    fn default() -> Self {
        Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            writes: Arc::new(Mutex::new(())),
            attempts: Arc::new(Mutex::new(VecDeque::new())),
            permits: Arc::new(Semaphore::new(4)),
        }
    }
}

#[derive(Clone, Serialize)]
struct Event {
    at: DateTime<Utc>,
    message: String,
}

struct Job {
    created: Instant,
    key_hash: String,
    node_id: Uuid,
    host: IpAddr,
    status: String,
    progress: u8,
    events: Vec<Event>,
}

impl Tasks {
    fn rate_limit(&self) -> Result<(), OperationsError> {
        let mut attempts = self.attempts.lock().map_err(|_| OperationsError::busy())?;
        attempts.retain(|at| at.elapsed() < Duration::from_secs(60));
        if attempts.len() >= 20 {
            return Err(OperationsError::busy());
        }
        attempts.push_back(Instant::now());
        Ok(())
    }

    fn event(&self, id: Uuid, progress: u8, status: &str, message: &str) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(job) = jobs.get_mut(&id) {
                if status != "failed" {
                    job.progress = progress;
                }
                job.status = status.to_owned();
                if job.events.len() < 128 {
                    job.events.push(Event {
                        at: Utc::now(),
                        message: message.to_owned(),
                    });
                }
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Probe {
    host: IpAddr,
    port: u16,
}

#[derive(Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AuthMethod {
    #[default]
    Password,
    PrivateKey,
}

#[derive(Default, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DeploymentMode {
    #[default]
    Basic,
    Full,
}

impl DeploymentMode {
    fn name(self) -> &'static str {
        match self { Self::Basic => "basic", Self::Full => "full" }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddMachine {
    host: IpAddr,
    port: u16,
    display_name: String,
    server_url: String,
    fingerprint: String,
    username: String,
    #[serde(default)]
    deployment_mode: DeploymentMode,
    #[serde(default, deserialize_with = "deserialize_secret")]
    sudo_password: Zeroizing<String>,
    #[serde(default)]
    auth_method: AuthMethod,
    #[serde(default, deserialize_with = "deserialize_secret")]
    password: Zeroizing<String>,
    #[serde(default, deserialize_with = "deserialize_secret")]
    private_key: Zeroizing<String>,
    #[serde(default, deserialize_with = "deserialize_secret")]
    passphrase: Zeroizing<String>,
}

impl AddMachine {
    fn clear_ssh_credentials(&mut self) {
        self.password.zeroize();
        self.private_key.zeroize();
        self.passphrase.zeroize();
    }

    fn validate_authentication(&self) -> Result<(), OperationsError> {
        let invalid = || {
            OperationsError::bad_request(
                "SSH 鉴权参数不正确：请选择密码或私钥一种方式；私钥仅支持 OpenSSH/PEM，最大 12 KiB",
            )
        };
        if self.sudo_password.len() > 1024
            || self.sudo_password.bytes().any(|byte| b"\r\n\0".contains(&byte))
            || (self.deployment_mode == DeploymentMode::Basic
                && (self.username == "root" || !self.sudo_password.is_empty()))
        {
            return Err(OperationsError::bad_request("普通用户模式不能使用 root 或 sudo 凭据；管理员口令不能包含换行"));
        }
        match self.auth_method {
            AuthMethod::Password => {
                if self.password.is_empty()
                    || self.password.len() > 1024
                    || self.password.bytes().any(|byte| b"\r\n\0".contains(&byte))
                    || !self.private_key.is_empty()
                    || !self.passphrase.is_empty()
                {
                    return Err(invalid());
                }
            }
            AuthMethod::PrivateKey => {
                let key = self.private_key.trim();
                let formats = [
                    "OPENSSH PRIVATE KEY",
                    "RSA PRIVATE KEY",
                    "EC PRIVATE KEY",
                    "DSA PRIVATE KEY",
                    "PRIVATE KEY",
                    "ENCRYPTED PRIVATE KEY",
                ];
                if !self.password.is_empty()
                    || key.is_empty()
                    || self.private_key.len() > MAX_PRIVATE_KEY_BYTES
                    || self.private_key.contains('\0')
                    || self.passphrase.contains('\0')
                    || self.passphrase.len() > 1024
                    || !formats.iter().any(|kind| {
                        key.starts_with(&format!("-----BEGIN {kind}-----"))
                            && key.ends_with(&format!("-----END {kind}-----"))
                    })
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}

fn authenticate(session: &Session, request: &mut AddMachine) -> Result<(), OperationsError> {
    let result = match request.auth_method {
        AuthMethod::Password => session.userauth_password(&request.username, &request.password),
        AuthMethod::PrivateKey => session.userauth_pubkey_memory(
            &request.username,
            None,
            &request.private_key,
            if request.passphrase.is_empty() {
                None
            } else {
                Some(&request.passphrase)
            },
        ),
    };
    // Clear Rust-owned copies immediately on both success and failure. libssh2's
    // internal buffers / the TLS/JSON stack are outside this zeroization guarantee.
    request.clear_ssh_credentials();
    result.map_err(|_| {
        OperationsError(
            StatusCode::UNAUTHORIZED,
            "SSH 认证失败，请检查所选方式、用户名、密码/私钥口令及目标机授权；凭据未保存",
        )
    })?;
    if !session.authenticated() {
        return Err(OperationsError::ssh("SSH 未完成认证"));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeMachine {
    action: String,
    display_name: String,
    note: String,
    confirmation: String,
    username: String,
    #[serde(default = "default_auth_scope")]
    auth_scope: String,
    #[serde(deserialize_with = "deserialize_secret")]
    password: Zeroizing<String>,
}

fn default_auth_scope() -> String {
    "target".to_owned()
}

pub(super) fn router() -> Router<OperationsState> {
    Router::new()
        .route("/api/v1/machines", get(list).post(add))
        .route("/api/v1/machines/probe", post(probe))
        .route("/api/v1/machines/jobs/{id}", get(job))
        .route("/api/v1/machines/{id}/manage", post(change))
}

fn validate_host(host: IpAddr, port: u16) -> Result<(), OperationsError> {
    let allowed = match host {
        IpAddr::V4(ip) => {
            let bytes = ip.octets();
            ip.is_private() || (bytes[0] == 100 && (64..=127).contains(&bytes[1]))
        }
        IpAddr::V6(ip) => ip.octets()[0] & 0xfe == 0xfc,
    };
    if !allowed || port == 0 {
        return Err(OperationsError::bad_request(
            "只允许内网/Tailscale/ULA IP 与有效 SSH 端口，不接受域名、回环或公网地址",
        ));
    }
    Ok(())
}

fn validate_label(name: &str, note: &str) -> Result<(), OperationsError> {
    if name.trim().is_empty()
        || name.len() > 160
        || name.chars().any(char::is_control)
        || note.len() > 2000
        || note.contains('\0')
    {
        return Err(OperationsError::bad_request("显示名称或备注格式不正确"));
    }
    Ok(())
}

fn fingerprint(session: &Session) -> Result<String, OperationsError> {
    let (key, _) = session
        .host_key()
        .ok_or(OperationsError::ssh("SSH 主机密钥不可用"))?;
    Ok(format!(
        "SHA256:{}",
        STANDARD_NO_PAD.encode(Sha256::digest(key))
    ))
}

fn handshake(host: IpAddr, port: u16) -> Result<Session, OperationsError> {
    let socket = TcpStream::connect_timeout(&SocketAddr::new(host, port), SSH_TIMEOUT)
        .map_err(|_| OperationsError::ssh("SSH 连接失败"))?;
    let mut session = Session::new().map_err(|_| OperationsError::ssh("SSH 初始化失败"))?;
    session.set_tcp_stream(socket);
    session.set_timeout(10_000);
    session
        .handshake()
        .map_err(|_| OperationsError::ssh("SSH 握手失败"))?;
    Ok(session)
}

async fn probe(
    State(state): State<OperationsState>,
    uri: Uri,
    headers: HeaderMap,
    Json(request): Json<Probe>,
) -> Result<Json<serde_json::Value>, OperationsError> {
    validate_origin(&headers, &uri)?;
    validate_host(request.host, request.port)?;
    state.machines.rate_limit()?;
    let permit = state
        .machines
        .permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| OperationsError::busy())?;
    let value = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        fingerprint(&handshake(request.host, request.port)?)
    })
    .await
    .map_err(|_| OperationsError::busy())??;
    Ok(Json(
        json!({"fingerprint":value, "message":"尚未发送密码。请与目标机 SSH 主机指纹核对，确认后才开始部署。"}),
    ))
}

async fn list(
    State(state): State<OperationsState>,
) -> Result<Json<serde_json::Value>, OperationsError> {
    let targets = load_all_targets(&state.registry)?;
    let platform_admin_available = platform_target(&state).is_ok();
    let rows = sqlx::query("SELECT id,hostname,display_name,management_note,deleted_at,last_seen FROM nodes ORDER BY hostname")
        .fetch_all(&state.pool).await.map_err(|_| OperationsError::busy())?;
    let machines: Vec<_> = rows.iter().map(|row| {
        let id: Uuid = row.get("id");
        let target = targets.iter().find(|target| target.node_id == id);
        json!({"node_id":id,"hostname":row.get::<String,_>("hostname"),
            "display_name":row.get::<Option<String>,_>("display_name"),
            "note":row.get::<String,_>("management_note"),"deleted_at":row.get::<Option<DateTime<Utc>>,_>("deleted_at"),
            "last_seen":row.get::<DateTime<Utc>,_>("last_seen"),
            "host":target.map(|t|t.host),"port":target.map(|t|t.port),
            "username":target.map(|t|t.default_user.clone()),"manageable":target.is_some() || platform_admin_available})
    }).collect();
    Ok(Json(
        json!({"machines":machines,"candidate_available":candidate().is_ok(),"ca_available":agent_ca(&state).is_ok(),
        "registry_writable":check_parent(&state.registry).is_ok()}),
    ))
}

async fn job(
    State(state): State<OperationsState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, OperationsError> {
    let key = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(OperationsError(
            StatusCode::UNAUTHORIZED,
            "任务访问凭据缺失",
        ))?;
    let jobs = state
        .machines
        .jobs
        .lock()
        .map_err(|_| OperationsError::busy())?;
    let job = jobs
        .get(&id)
        .filter(|job| job.created.elapsed() < JOB_TTL)
        .ok_or(OperationsError(
            StatusCode::NOT_FOUND,
            "任务不存在或已过期；请检查机器心跳",
        ))?;
    if !crate::api::constant_time_equal(&job.key_hash, &crate::api::hash_token(key)) {
        return Err(OperationsError(
            StatusCode::UNAUTHORIZED,
            "任务访问凭据无效",
        ));
    }
    Ok(Json(
        json!({"node_id":job.node_id,"status":job.status,"progress":job.progress,"events":job.events}),
    ))
}

async fn add(
    State(state): State<OperationsState>,
    uri: Uri,
    headers: HeaderMap,
    payload: Result<Json<AddMachine>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<serde_json::Value>, OperationsError> {
    validate_origin(&headers, &uri)?;
    // Do not echo malformed field values (which may contain a secret) in JSON errors.
    let Json(mut request) = payload.map_err(|error| {
        OperationsError(
            if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                StatusCode::PAYLOAD_TOO_LARGE
            } else {
                StatusCode::BAD_REQUEST
            },
            "新增机器请求格式或大小不正确，请检查鉴权参数；请求内容不会回显",
        )
    })?;
    request.server_url = controller_address(&request.server_url)?;
    validate_host(request.host, request.port)?;
    validate_label(&request.display_name, "")?;
    request.validate_authentication()?;
    if !valid_username(&request.username)
        || !request
            .username
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        || !valid_agent_home(
            &deployment_home(&request.username).join(".easymanage"),
            &request.username,
        )
        || !valid_controller_url(&request.server_url)
    {
        return Err(OperationsError::bad_request(
            "请使用有效 SSH 账户及带端口的 HTTPS 平台地址；普通用户 home 需为 /home/账户，root 为 /root",
        ));
    }
    check_parent(&state.registry)?;
    let _ = candidate()?;
    let _ = agent_ca(&state)?;
    state.machines.rate_limit()?;
    let targets = load_all_targets(&state.registry)?;
    if targets.len() >= 128 {
        return Err(OperationsError::bad_request("机器数量达到 128 个安全上限"));
    }
    if targets.iter().any(|t| t.host == request.host) {
        return Err(OperationsError::bad_request(
            "该 IP 已受管；已删除机器请恢复，不要重复安装新身份",
        ));
    }
    let permit = state
        .machines
        .permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| OperationsError::busy())?;
    let id = Uuid::new_v4();
    let key = Zeroizing::new(format!(
        "{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    ));
    {
        let mut jobs = state
            .machines
            .jobs
            .lock()
            .map_err(|_| OperationsError::busy())?;
        jobs.retain(|_, job| job.created.elapsed() < JOB_TTL || job.status == "running");
        if jobs.len() >= 32
            || jobs
                .values()
                .any(|job| job.host == request.host && job.status == "running")
        {
            return Err(OperationsError::busy());
        }
        jobs.insert(
            id,
            Job {
                created: Instant::now(),
                key_hash: crate::api::hash_token(&key),
                node_id: id,
                host: request.host,
                status: "running".to_owned(),
                progress: 0,
                events: vec![],
            },
        );
    }
    let response = json!({"job_id":id,"job_key":key.as_str(),"node_id":id});
    let actor = request.username.clone();
    let deployment_mode = request.deployment_mode;
    state.machines.event(
        id,
        0,
        "running",
        if deployment_mode == DeploymentMode::Basic { "任务已创建；普通用户模式，所有安装文件位于 SSH 用户 home，不执行 sudo" }
        else { "任务已创建；已选择首次安装管理员 Full Agent，将验证 root/sudo 权限" },
    );
    tokio::spawn(async move {
        let _permit = permit;
        let worker_state = state.clone();
        let handle = tokio::runtime::Handle::current();
        let worker =
            tokio::task::spawn_blocking(move || deploy(&worker_state, id, request, &handle));
        let result = worker.await;
        let outcome = if matches!(&result, Ok(Ok(()))) {
            "success"
        } else {
            "failed"
        };
        if audit(&state.pool, id, &actor, "machine.add.result", outcome)
            .await
            .is_err()
        {
            tracing::error!(%id,"machine deployment result audit unavailable");
        }
        match result {
            Ok(Ok(())) => {
                state
                    .machines
                    .event(id, 100, "success", if deployment_mode == DeploymentMode::Basic { "已收到新 Agent 的 Basic 心跳，部署完成；后续可通过设备总览提权" } else { "已收到新 Agent 的 Full / UID 0 心跳，管理员部署完成" })
            }
            Ok(Err(error)) => state.machines.event(id, 100, "failed", error.1),
            Err(_) => state.machines.event(
                id,
                100,
                "failed",
                "部署任务异常；不推断已回滚，请核对目标用户/系统服务与原节点身份",
            ),
        }
    });
    Ok(Json(response))
}

fn controller_address(value: &str) -> Result<String, OperationsError> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| OperationsError::bad_request("平台地址不是有效 HTTPS 地址"))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(OperationsError::bad_request(
            "平台地址只填写 HTTPS 主机和端口，不包含账号、页面路径或参数",
        ));
    }
    let host = url
        .host_str()
        .ok_or(OperationsError::bad_request("平台地址缺少主机"))?;
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    let result = format!(
        "https://{host}:{}",
        url.port_or_known_default().unwrap_or(443)
    );
    if !valid_controller_url(&result) {
        return Err(OperationsError::bad_request("平台 HTTPS 地址格式不正确"));
    }
    Ok(result)
}

fn candidate() -> Result<PathBuf, OperationsError> {
    let path = release_root()?.join("target/release/easymanage-agent");
    check_release_file(&path).map_err(|_| {
        OperationsError::bad_request("最新 Agent 候选尚未发布；请先在平台构建并发布")
    })?;
    Ok(path)
}

fn agent_ca(state: &OperationsState) -> Result<Vec<u8>, OperationsError> {
    let path = std::env::var_os("EASYMANAGE_AGENT_CA_CERTIFICATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            state
                .config
                .tls_certificate
                .parent()
                .unwrap_or(FsPath::new("."))
                .join("ca.crt")
        });
    check_private_file(&path).map_err(|_| {
        OperationsError::bad_request(
            "Agent 信任 CA 不可读取；请配置 EASYMANAGE_AGENT_CA_CERTIFICATE",
        )
    })?;
    let bytes = fs::read(path).map_err(|_| OperationsError::registry())?;
    reqwest::Certificate::from_pem(&bytes)
        .map_err(|_| OperationsError::bad_request("Agent CA 不是有效 PEM 证书"))?;
    Ok(bytes)
}

fn command(session: &Session, invocation: &str) -> Result<String, OperationsError> {
    let mut channel = session
        .channel_session()
        .map_err(|_| OperationsError::ssh("SSH 通道不可用"))?;
    channel
        .exec(invocation)
        .map_err(|_| OperationsError::ssh("远端检查失败"))?;
    let mut output = Vec::new();
    (&mut channel)
        .take(8193)
        .read_to_end(&mut output)
        .map_err(|_| OperationsError::ssh("远端检查超时或中断"))?;
    if output.len() > 8192 {
        return Err(OperationsError::ssh("远端检查输出超过上限"));
    }
    channel
        .wait_close()
        .map_err(|_| OperationsError::ssh("远端通道中断"))?;
    if channel.exit_status().unwrap_or(1) != 0 {
        return Err(OperationsError::ssh(
            "远端环境检查失败；请核对 Linux、Python3、目标账户及所选部署权限；Basic 支持无 linger 的兼容模式",
        ));
    }
    String::from_utf8(output).map_err(|_| OperationsError::ssh("远端检查格式异常"))
}

fn check_parent(registry: &FsPath) -> Result<(), OperationsError> {
    if !registry.is_absolute() {
        return Err(OperationsError::bad_request("可信清单需使用绝对路径"));
    }
    let parent = registry.parent().ok_or(OperationsError::registry())?;
    let meta = fs::symlink_metadata(parent).map_err(|_| OperationsError::registry())?;
    let uid = fs::metadata("/proc/self")
        .map_err(|_| OperationsError::registry())?
        .uid();
    if !meta.is_dir()
        || meta.permissions().mode() & 0o022 != 0
        || meta.uid() != uid
        || meta.permissions().mode() & 0o300 != 0o300
    {
        return Err(OperationsError::bad_request(
            "可信清单目录必须归平台服务用户所有且不可由组/其他用户写入；请管理员配置后再添加",
        ));
    }
    let root_uid = fs::metadata("/")
        .map_err(|_| OperationsError::registry())?
        .uid();
    for ancestor in parent.ancestors() {
        let entry = fs::symlink_metadata(ancestor).map_err(|_| OperationsError::registry())?;
        let owned = entry.uid() == uid || entry.uid() == root_uid;
        // Shared home/.local permissions are an operator decision. Only the actual
        // registry directory above must be private; ancestor group-write is allowed.
        let public_write =
            entry.permissions().mode() & 0o002 != 0 && entry.permissions().mode() & 0o1000 == 0;
        if !entry.is_dir() || !owned || public_write {
            return Err(OperationsError::bad_request(
                "可信清单路径包含符号链接、不可信所有者或公开可写目录",
            ));
        }
    }
    Ok(())
}

fn save_target(state: &OperationsState, target: &Target) -> Result<(), OperationsError> {
    let _lock = state
        .machines
        .writes
        .lock()
        .map_err(|_| OperationsError::busy())?;
    check_parent(&state.registry)?;
    let path = state.registry.with_file_name("managed-targets.json");
    let mut targets = read_target_file(&path)?;
    let all = load_all_targets(&state.registry)?;
    if !all.iter().any(|old| old.node_id == target.node_id) && all.len() >= 128 {
        return Err(OperationsError::bad_request("机器数量达到上限"));
    }
    if all
        .iter()
        .any(|old| old.node_id != target.node_id && old.host == target.host)
    {
        return Err(OperationsError::bad_request(
            "同一 IP 已关联其他机器，不允许重复部署",
        ));
    }
    targets.retain(|old| old.node_id != target.node_id);
    targets.push(target.clone());
    let bytes = serde_json::to_vec_pretty(&json!({"targets":targets}))
        .map_err(|_| OperationsError::registry())?;
    let temporary = path.with_file_name(format!(".managed-targets-{}.next", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|_| OperationsError::registry())?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| OperationsError::registry())?;
    fs::rename(&temporary, &path).map_err(|_| OperationsError::registry())?;
    Ok(())
}

fn upload(
    session: &Session,
    path: &FsPath,
    bytes: &[u8],
    mode: i32,
) -> Result<(), OperationsError> {
    let mut stream = session
        .scp_send(path, mode, bytes.len() as u64, None)
        .map_err(|_| OperationsError::ssh("上传暂存文件失败"))?;
    stream
        .write_all(bytes)
        .map_err(|_| OperationsError::ssh("文件上传中断"))?;
    stream
        .send_eof()
        .and_then(|()| stream.wait_eof())
        .and_then(|()| stream.close())
        .and_then(|()| stream.wait_close())
        .map_err(|_| OperationsError::ssh("上传校验通道中断"))?;
    Ok(())
}

fn deployment_home(username: &str) -> PathBuf {
    if username == "root" { PathBuf::from("/root") } else { PathBuf::from(format!("/home/{username}")) }
}

// Only fixed controller-owned scripts are executed as root. Secrets go over
// channel stdin, never command arguments, remote files or job logs.
fn admin_command(session: &Session, request: &AddMachine, invocation: &str) -> Result<(), OperationsError> {
    let invocation = if request.username == "root" { invocation.to_owned() }
        else { format!("/usr/bin/sudo -k -S -p '' {invocation}") };
    let mut channel = session.channel_session().map_err(|_| OperationsError::ssh("管理员通道不可用"))?;
    channel.handle_extended_data(ExtendedData::Merge).map_err(|_| OperationsError::ssh("管理员通道初始化失败"))?;
    channel.exec(&invocation).map_err(|_| OperationsError::ssh("管理员操作启动失败"))?;
    if request.username != "root" {
        channel.write_all(request.sudo_password.as_bytes()).and_then(|()| channel.write_all(b"\n"))
            .map_err(|_| OperationsError::ssh("sudo 认证提交失败"))?;
    }
    channel.send_eof().map_err(|_| OperationsError::ssh("管理员通道已中断"))?;
    session.set_blocking(false);
    let result = (|| {
        let started = Instant::now();
        let mut output = Zeroizing::new([0_u8; 4096]);
        let mut total = 0;
        loop {
            if started.elapsed() > Duration::from_secs(100) { return Err(OperationsError::ssh("管理员安装超时；请核对目标服务及保留的恢复文件")); }
            match channel.read(&mut *output) {
                Ok(size) => total += size,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {},
                Err(_) => return Err(OperationsError::ssh("管理员通道中断；请核对目标服务")),
            }
            if total > 64 * 1024 { return Err(OperationsError::ssh("管理员操作输出超过限制")); }
            if channel.eof() { break; }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    })();
    session.set_blocking(true);
    session.set_timeout(10_000);
    result?;
    channel.wait_close().map_err(|_| OperationsError::ssh("管理员通道关闭失败"))?;
    if channel.exit_status().unwrap_or(1) != 0 { return Err(OperationsError::ssh("管理员操作失败；请检查 root/sudo 授权、目标服务日志和恢复文件；凭据不会保存")); }
    Ok(())
}

fn deploy(
    state: &OperationsState,
    id: Uuid,
    mut request: AddMachine,
    handle: &tokio::runtime::Handle,
) -> Result<(), OperationsError> {
    state.machines.event(
        id,
        5,
        "running",
        "连接 SSH 并核验已确认的主机指纹（尚未进行身份鉴权）",
    );
    let session = handshake(request.host, request.port)?;
    if fingerprint(&session)? != request.fingerprint {
        return Err(OperationsError::ssh(
            "SSH 指纹与确认值不同；未进行身份鉴权，已停止部署",
        ));
    }
    let mode = request.deployment_mode;
    if mode == DeploymentMode::Full && request.username != "root" && request.sudo_password.is_empty()
        && request.auth_method == AuthMethod::Password {
        request.sudo_password = Zeroizing::new(request.password.to_string());
    }
    authenticate(&session, &mut request)?;
    state.machines.event(
        id,
        10,
        "running",
        if mode == DeploymentMode::Basic { "SSH 认证成功；检查普通用户身份、Linux 架构与 Python3；不强制要求 linger" }
        else { "SSH 认证成功；检查 Linux 架构和管理员 root/sudo 权限（Full 不依赖用户 linger）" },
    );
    let info = command(
        &session,
        &format!("/bin/bash --noprofile --norc -c {}", shell_quote(&format!(
            "set -eu; test \"$(uname -s)\" = Linux; command -v python3 >/dev/null; command -v timeout >/dev/null; printf '%s\\n' \"$(id -un)\" \"$(getent passwd \"$(id -un)\" | cut -d: -f6)\" \"$(uname -m)\" \"$(id -u)\"; {}",
            if mode == DeploymentMode::Basic { "test \"$(id -u)\" != 0" }
            else { "systemctl show-environment >/dev/null" }
        ))),
    )?;
    let fields: Vec<_> = info.lines().collect();
    if fields.len() != 4
        || fields[0] != request.username
        || PathBuf::from(fields[1]) != deployment_home(&request.username)
    {
        return Err(OperationsError::ssh(
            "目标用户名/home 与请求不符；普通账户需 /home/账户，root 需 /root",
        ));
    }
    let uid: u32 = fields[3]
        .parse()
        .map_err(|_| OperationsError::ssh("目标 UID 不正确"))?;
    if (mode == DeploymentMode::Basic && uid == 0) || (request.username == "root") != (uid == 0) {
        return Err(OperationsError::ssh("目标 UID 与所选账户/部署权限不一致"));
    }
    if mode == DeploymentMode::Full {
        admin_command(&session, &request, "/usr/bin/true")?;
    }
    let binary_path = candidate()?;
    let binary = fs::read(&binary_path).map_err(|_| OperationsError::registry())?;
    let hash = hex::encode(Sha256::digest(&binary));
    if !compatible_binary(&binary, fields[2]) {
        return Err(OperationsError::bad_request(
            "候选 Agent 与目标 CPU 架构不匹配；未安装任何文件",
        ));
    }
    // Refuse an existing Agent, even if stopped. Onboarding never reassigns its UUID.
    command(&session, "test ! -e \"$HOME/.local/share/easymanage/config/agent.env\" && test ! -e \"$HOME/.local/share/easymanage/state/state.db\"")
        .map_err(|_| OperationsError::bad_request("检测到旧用户目录 Agent 身份，请先迁移到 ~/.easymanage，不允许重复注册"))?;
    command(&session,"/bin/bash --noprofile --norc -c 'set -eu; if command -v systemctl >/dev/null; then state=$(systemctl show easymanage-agent.service -p LoadState --value 2>/dev/null || true); test -z \"$state\" || test \"$state\" = not-found; fi; test ! -e /etc/systemd/system/easymanage-agent.service; test ! -d /var/lib/easymanage-agent; test ! -e /usr/local/sbin/easymanage-agent; test ! -d /etc/easymanage || test -x /etc/easymanage; test ! -e /etc/easymanage/agent.env; test ! -e \"$HOME/.easymanage/state/state.db\"; test ! -e \"$HOME/.easymanage/config/agent.env\"; test ! -e \"$HOME/.config/systemd/user/easymanage-agent.service\"' ")
        .map_err(|_|OperationsError::bad_request("目标已存在 Agent 配置、用户单元或身份库；请恢复/接管原节点，不允许覆盖安装"))?;
    let bootstrap = Zeroizing::new(format!(
        "{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    ));
    handle.block_on(async {
        let mut tx=state.pool.begin().await.map_err(|_|OperationsError::busy())?;
        sqlx::query("INSERT INTO nodes(id,hostname,display_name,os_name,os_version,kernel_version,architecture,cpu_model,cpu_logical_cores,agent_version,last_seen) VALUES($1,$2,$2,'等待 Agent','','',$3,'',0,'等待 Agent',NOW()-INTERVAL '1 day')")
            .bind(id).bind(&request.display_name).bind(fields[2]).execute(&mut *tx).await.map_err(|_|OperationsError::busy())?;
        sqlx::query("INSERT INTO machine_enrollments(node_id,token_hash,expires_at,expected_user) VALUES($1,$2,NOW()+INTERVAL '10 minutes',$3)")
            .bind(id).bind(crate::api::hash_token(&bootstrap)).bind(&request.username).execute(&mut *tx).await.map_err(|_|OperationsError::busy())?;
        sqlx::query("INSERT INTO audit_logs(actor_type,actor_id,action,target_type,target_id,details) VALUES('ssh-user',$1,'machine.add','node',$2,'{\"result\":\"authorized\"}')")
            .bind(&request.username).bind(id.to_string()).execute(&mut *tx).await.map_err(|_|OperationsError::busy())?;
        tx.commit().await.map_err(|_|OperationsError::busy())
    })?;
    state.machines.event(
        id,
        15,
        "running",
        "已为新机器预留 ID；准备 Agent CA 与已核验的 SSH 公钥",
    );
    let home = PathBuf::from(fields[1]).join(".easymanage");
    // Deploy the Agent's trusted CA, not the platform leaf certificate.
    let ca = agent_ca(state)?;
    let key_path = state
        .registry
        .with_file_name(format!("machine-{id}.known_hosts"));
    check_parent(&state.registry)?;
    let mut known = session
        .known_hosts()
        .map_err(|_| OperationsError::registry())?;
    let (key, kind) = session.host_key().ok_or(OperationsError::registry())?;
    let host_label = if request.port == 22 {
        request.host.to_string()
    } else {
        format!("[{}]:{}", request.host, request.port)
    };
    known
        .add(
            &host_label,
            key,
            "EasyManage confirmed onboarding",
            kind.into(),
        )
        .map_err(|_| OperationsError::ssh("SSH 公钥格式不受支持，未继续安装"))?;
    // UUID filename is new; never replace another target's trust material.
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&key_path)
        .map_err(|_| OperationsError::ssh("无法创建私有 SSH 公钥文件，未继续安装"))?;
    known
        .write_file(&key_path, KnownHostFileKind::OpenSSH)
        .map_err(|_| OperationsError::ssh("无法保存已核验 SSH 公钥，未继续安装"))?;
    let mut target = Target {
        node_id: id,
        hostname: request.display_name.clone(),
        host: request.host,
        port: request.port,
        default_user: request.username.clone(),
        agent_home: home.clone(),
        known_hosts: key_path,
        agent_sha256: None,
        server_url: Some(request.server_url.clone()),
        ca_sha256: Some(hex::encode(Sha256::digest(&ca))),
        release_sha256: Some(hash),
        ssh_alias: None,
        disabled: false,
    };
    save_target(state, &target)?;
    state.machines.event(
        id,
        25,
        "running",
        "机器身份与单机限时注册凭据已准备；创建私有暂存目录",
    );
    let stage = command(
        &session,
        "umask 077; test ! -L \"$HOME/.easymanage\"; test ! -L \"$HOME/.easymanage/staging\"; mkdir -p \"$HOME/.easymanage/staging\"; chmod 700 \"$HOME/.easymanage\" \"$HOME/.easymanage/staging\"; mktemp -d \"$HOME/.easymanage/staging/agent.XXXXXXXX\"",
    )?
    .trim()
    .to_owned();
    let stage_prefix = format!("{}/.easymanage/staging/agent.", fields[1]);
    if !stage.starts_with(&stage_prefix)
        || stage[stage_prefix.len()..].len() != 8
        || !stage[stage_prefix.len()..]
            .bytes()
            .all(|c| c.is_ascii_alphanumeric())
    {
        return Err(OperationsError::ssh("暂存路径不安全"));
    }
    let env = Zeroizing::new(format!(
        "EASYMANAGE_SERVER_URL={}\nEASYMANAGE_CA_CERTIFICATE={}/config/ca.crt\nEASYMANAGE_AGENT_STATE_DB={}/state/state.db\nEASYMANAGE_ENROLLMENT_TOKEN={}\nEASYMANAGE_NODE_ID={}\nEASYMANAGE_REPORT_INTERVAL_SECONDS=10\n",
        request.server_url,
        home.display(),
        home.display(),
        bootstrap.as_str(),
        id
    ));
    let node = serde_json::to_vec(&json!({"node_id":id,"user":request.username,"agent_home":home}))
        .map_err(|_| OperationsError::registry())?;
    let files: Vec<(&str, &[u8], i32)> = vec![
        ("easymanage-agent", &binary, 0o700),
        ("install-user-agent.sh", USER_INSTALL.as_bytes(), 0o700),
        ("agent-user-control.py", USER_CONTROL.as_bytes(), 0o700),
        ("easymanage-agent.user.service", USER_UNIT.as_bytes(), 0o600),
        ("node.json", &node, 0o600),
        ("agent.env", env.as_bytes(), 0o600),
        ("ca.crt", &ca, 0o600),
    ];
    let mut manifest = String::new();
    for (index, (name, bytes, mode)) in files.into_iter().enumerate() {
        upload(&session, &PathBuf::from(&stage).join(name), bytes, mode)?;
        manifest.push_str(&format!("{}  {name}\n", hex::encode(Sha256::digest(bytes))));
        state.machines.event(
            id,
            30 + (index as u8) * 4,
            "running",
            &format!("已上传 {name}（{} 字节，权限受限）", bytes.len()),
        );
    }
    upload(
        &session,
        &PathBuf::from(&stage).join("SHA256SUMS"),
        manifest.as_bytes(),
        0o600,
    )?;
    state.machines.event(
        id,
        55,
        "running",
        "最新候选与配置上传完成；远端将逐文件核对 SHA-256",
    );
    state.machines.event(
        id,
        60,
        "running",
        if mode == DeploymentMode::Basic { "安装并启动 Basic Agent：优先 systemd + linger，否则自动使用用户兼容守护；不调用 sudo" }
        else { "以 root/sudo 安装系统服务并直接启动 Full Agent；不创建 Basic 用户服务" },
    );
    let invocation = format!(
        "/usr/bin/timeout --signal=TERM --kill-after=3s 90s /bin/bash {}",
        shell_quote(&format!("{stage}/install-user-agent.sh"))
    );
    session.set_timeout(100_000);
    if mode == DeploymentMode::Basic { command(&session, &invocation).map_err(|_| {
        OperationsError::ssh(
            "Basic 安装/启动失败；请检查 ~/.easymanage 下的日志、目标执行策略与保留的暂存包；无需先启用 linger",
        )
    })?;
        let launch = command(&session, &format!("python3 {} describe", shell_quote(&home.join("bin/agent-user-control.py").to_string_lossy())))?;
        let launch: serde_json::Value = serde_json::from_str(&launch).map_err(|_| OperationsError::ssh("启动方式信息格式错误，未确认部署成功"))?;
        let message = match (launch["backend"].as_str(), launch["autostart"].as_str()) {
            (Some("systemd"), _) => "已使用 systemd 用户服务（现有 linger 已启用）；未修改系统策略",
            (Some("compatibility"), Some("cron")) => "已使用无 linger 兼容守护；已添加用户 cron 开机/每分钟恢复任务，依赖目标 cron 服务运行",
            (Some("compatibility"), _) => "已使用无 linger 兼容守护；用户 cron 不可用，无法保证开机/退出会话后自动恢复；请勿视为系统服务保证",
            _ => return Err(OperationsError::ssh("未知 Basic 启动方式，未确认部署成功")),
        };
        state.machines.event(id, 75, "running", message);
    } else {
        let invocation = format!("/usr/bin/timeout --signal=TERM --kill-after=3s 90s /usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin /bin/bash --noprofile --norc -c {} -- {} {} {} {} {} {}",
            shell_quote(FULL_INSTALL), shell_quote(&stage), shell_quote(&target.release_sha256.clone().unwrap_or_default()),
            shell_quote(target.ca_sha256.as_deref().unwrap_or_default()), shell_quote(&hex::encode(Sha256::digest(env.as_bytes()))),
            shell_quote(&id.to_string()), shell_quote(&request.server_url));
        let result = admin_command(&session, &request, &invocation);
        request.sudo_password.zeroize();
        result?;
    }
    target.agent_sha256 = target.release_sha256.clone();
    save_target(state, &target)?;
    state.machines.event(
        id,
        80,
        "running",
        if mode == DeploymentMode::Basic { "普通用户 Agent 已启动；等待真实 Basic 心跳，未确认前不标记成功" }
        else { "系统服务已启动；等待真实 Full / UID 0 心跳，未确认前不标记成功" },
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        let confirmed=handle.block_on(async {
            sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM nodes n JOIN metrics_latest m ON m.node_id=n.id WHERE n.id=$1 AND n.deleted_at IS NULL AND n.last_seen>NOW()-INTERVAL '30 seconds' AND m.metrics->'privilege'->>'mode'=$2 AND m.metrics->'privilege'->>'uid'=$3)")
                .bind(id).bind(mode.name()).bind(if mode == DeploymentMode::Basic { uid.to_string() } else { "0".to_owned() }).fetch_one(&state.pool).await
        }).unwrap_or(false);
        if confirmed {
            // Remove only the exact uploaded files, never recursive cleanup.
            let cleanup = format!(
                "cd {} && unlink SHA256SUMS && unlink easymanage-agent && unlink install-user-agent.sh && unlink agent-user-control.py && unlink easymanage-agent.user.service && unlink node.json && unlink agent.env && unlink ca.crt && rmdir {}",
                shell_quote(&stage),
                shell_quote(&stage)
            );
            if command(&session, &cleanup).is_err() {
                state.machines.event(
                    id,
                    95,
                    "running",
                    "心跳已确认，但暂存包清理未完成；请核对该 SSH 用户 home 中的私有暂存包",
                );
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    Err(OperationsError::ssh(
        "服务已启动但 60 秒内未收到心跳；保留原机器 ID 和暂存包，请检查平台 URL、CA 及目标连接",
    ))
}

fn compatible_binary(bytes: &[u8], architecture: &str) -> bool {
    bytes.len() >= 20
        && &bytes[..4] == b"\x7fELF"
        && bytes[4] == 2
        && bytes[5] == 1
        && matches!(
            (u16::from_le_bytes([bytes[18], bytes[19]]), architecture),
            (62, "x86_64") | (183, "aarch64")
        )
}

async fn change(
    State(state): State<OperationsState>,
    Path(id): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
    Json(request): Json<ChangeMachine>,
) -> Result<Json<serde_json::Value>, OperationsError> {
    validate_origin(&headers, &uri)?;
    validate_label(&request.display_name, &request.note)?;
    if !matches!(request.action.as_str(), "edit" | "delete" | "restore")
        || request.confirmation != id.to_string()
    {
        return Err(OperationsError::bad_request("操作或机器确认 ID 不正确"));
    }
    state.machines.rate_limit()?;
    if state
        .machines
        .jobs
        .lock()
        .map_err(|_| OperationsError::busy())?
        .values()
        .any(|job| job.node_id == id && job.status == "running")
    {
        return Err(OperationsError::bad_request(
            "该机器仍在部署，完成后再编辑/删除/恢复",
        ));
    }
    let mut target = load_all_targets(&state.registry)?
        .into_iter()
        .find(|target| target.node_id == id);
    if request.action == "delete"
        && target
            .as_ref()
            .is_some_and(|target| target.ssh_alias.as_deref() == Some("local"))
    {
        return Err(OperationsError::bad_request(
            "平台本机节点受保护，不能从此处软删除",
        ));
    }
    let credentials = Credentials {
        username: request.username,
        password: request.password,
    };
    let actor = credentials.username.clone();
    validate_credentials(&credentials)?;
    let admin = request.auth_scope == "platform_admin";
    if !matches!(request.auth_scope.as_str(), "target" | "platform_admin") {
        return Err(OperationsError::bad_request("鉴权目标不正确"));
    }
    let check = if admin {
        platform_target(&state)?
    } else {
        target.clone().ok_or(OperationsError::bad_request(
            "机器缺少可信目标；请使用平台管理员鉴权",
        ))?
    };
    let permit = state
        .machines
        .permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| OperationsError::busy())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let session = ssh_connect(&check, &credentials, &Cancellation::default())?;
        if admin || credentials.username != check.default_user {
            verify_admin(&session, &credentials)?;
        }
        drop(session);
        Ok::<(), OperationsError>(())
    })
    .await
    .map_err(|_| OperationsError::busy())??;
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|_| OperationsError::busy())?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|_| OperationsError::busy())?;
    let deleted: Option<bool> =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM nodes WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| OperationsError::busy())?;
    let deleted = deleted.ok_or(OperationsError::bad_request("机器不存在"))?;
    if let Some(target) = target.as_mut() {
        target.disabled = deleted;
    }
    if request.action == "edit" {
        sqlx::query(
            "UPDATE nodes SET display_name=$2,management_note=$3,updated_at=NOW() WHERE id=$1",
        )
        .bind(id)
        .bind(&request.display_name)
        .bind(&request.note)
        .execute(&mut *tx)
        .await
        .map_err(|_| OperationsError::busy())?;
        if let Some(target) = target.as_mut() {
            target.hostname = request.display_name.clone();
        }
    } else {
        let remove = request.action == "delete";
        if deleted == remove {
            return Err(OperationsError::bad_request("机器状态已变化，请刷新后重试"));
        }
        sqlx::query("UPDATE nodes SET deleted_at=CASE WHEN $2 THEN NOW() ELSE NULL END,updated_at=NOW() WHERE id=$1")
            .bind(id).bind(remove).execute(&mut *tx).await.map_err(|_|OperationsError::busy())?;
        sqlx::query("UPDATE agents SET enabled=NOT $2 WHERE node_id=$1")
            .bind(id)
            .bind(remove)
            .execute(&mut *tx)
            .await
            .map_err(|_| OperationsError::busy())?;
        if let Some(target) = target.as_mut() {
            target.disabled = remove;
        }
    }
    sqlx::query("INSERT INTO audit_logs(actor_type,actor_id,action,target_type,target_id,details) VALUES('ssh-user',$1,$2,'node',$3,'{}')")
        .bind(&actor).bind(format!("machine.{}",request.action)).bind(id.to_string()).execute(&mut *tx).await.map_err(|_|OperationsError::busy())?;
    if let Some(target) = target.as_ref() {
        save_target(&state, target)?;
    }
    tx.commit().await.map_err(|_| OperationsError::busy())?;
    Ok(Json(
        json!({"message":match request.action.as_str(){"delete"=>"已软删除：历史、身份和远端文件保留，上报凭据停用", "restore"=>"已恢复原机器身份与上报凭据，等待 Agent 心跳", _=>"显示名称和备注已更新；SSH 地址、权限和机器身份未改变"}}),
    ))
}

fn verify_admin(session: &Session, credentials: &Credentials) -> Result<(), OperationsError> {
    if command(session, "/usr/bin/id -u")?.trim() == "0" {
        return Ok(());
    }
    let mut channel = session
        .channel_session()
        .map_err(|_| OperationsError::ssh("管理员认证通道失败"))?;
    channel
        .exec("/usr/bin/sudo -k -S -p '' /usr/bin/true")
        .map_err(|_| OperationsError::ssh("管理员验证失败"))?;
    channel
        .write_all(credentials.password.as_bytes())
        .and_then(|()| channel.write_all(b"\n"))
        .map_err(|_| OperationsError::ssh("管理员验证失败"))?;
    channel
        .send_eof()
        .map_err(|_| OperationsError::ssh("管理员验证失败"))?;
    let mut ignored = Vec::new();
    (&mut channel)
        .take(4096)
        .read_to_end(&mut ignored)
        .map_err(|_| OperationsError::ssh("管理员验证超时"))?;
    ignored.zeroize();
    channel
        .wait_close()
        .map_err(|_| OperationsError::ssh("管理员验证中断"))?;
    if channel.exit_status().unwrap_or(1) != 0 {
        return Err(OperationsError(
            StatusCode::UNAUTHORIZED,
            "需要管理员 sudo 权限",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn authentication_request(fields: serde_json::Value) -> AddMachine {
        let mut value = json!({"host":"10.0.0.9","port":22,"display_name":"fixture",
            "server_url":"https://controller:8443","fingerprint":"fixture","username":"tester"});
        for (name, field) in fields.as_object().unwrap() {
            value[name] = field.clone();
        }
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn password_legacy_default_and_private_key_requests_are_supported() {
        let request = authentication_request(json!({"password":"fixture-only"}));
        assert!(request.auth_method == AuthMethod::Password);
        assert!(request.deployment_mode == DeploymentMode::Basic);
        assert!(request.validate_authentication().is_ok());
        for kind in [
            "OPENSSH PRIVATE KEY",
            "RSA PRIVATE KEY",
            "EC PRIVATE KEY",
            "ENCRYPTED PRIVATE KEY",
        ] {
            let key = format!("-----BEGIN {kind}-----\nfixture-only\n-----END {kind}-----\n");
            let request = authentication_request(
                json!({"auth_method":"private_key", "private_key":key,"passphrase":"fixture-only"}),
            );
            assert!(request.validate_authentication().is_ok());
        }
    }

    #[test]
    fn mixed_missing_public_or_oversized_keys_are_rejected_without_echoing_secrets() {
        for fields in [
            json!({}),
            json!({"password":"fixture-secret", "private_key":"fixture-secret"}),
            json!({"auth_method":"private_key","private_key":"ssh-ed25519 fixture-secret"}),
            json!({"auth_method":"private_key","private_key":"fixture-secret".repeat(2000)}),
            json!({"password":"fixture-secret\u{0000}"}),
        ] {
            let request = authentication_request(fields);
            let error = request.validate_authentication().err().unwrap();
            assert!(!error.1.contains("fixture-secret"));
        }
    }

    #[test]
    fn deployment_modes_and_sudo_credentials_are_explicit() {
        for fields in [json!({"username":"root","password":"fixture-only"}),
            json!({"password":"fixture-only","sudo_password":"fixture-only"}),
            json!({"deployment_mode":"full","password":"fixture-only","sudo_password":"fixture\nsecret"})] {
            assert!(authentication_request(fields).validate_authentication().is_err());
        }
        for username in ["tester", "root"] {
            let request = authentication_request(json!({"deployment_mode":"full","username":username,"password":"fixture-only"}));
            assert!(request.validate_authentication().is_ok());
            assert!(request.deployment_mode == DeploymentMode::Full);
            assert!(valid_agent_home(&deployment_home(username).join(".easymanage"), username));
        }
        assert!(serde_json::from_value::<AddMachine>(json!({"deployment_mode":"unknown"})).is_err());
    }

    #[test]
    fn owned_credentials_are_cleared_together() {
        let mut request = authentication_request(
            json!({"password":"fixture-only","private_key":"fixture-only","passphrase":"fixture-only"}),
        );
        request.clear_ssh_credentials();
        assert!(
            request.password.is_empty()
                && request.private_key.is_empty()
                && request.passphrase.is_empty()
        );
    }

    #[test]
    fn shared_home_does_not_disable_a_private_registry_directory() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = std::env::temp_dir().join(format!("machine-permissions-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o2770)).unwrap();
        let config = root.join("config");
        fs::create_dir(&config).unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o700)).unwrap();
        let registry = config.join("targets.json");
        assert!(check_parent(&registry).is_ok());
        fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(check_parent(&registry).is_err());
        fs::set_permissions(&root, fs::Permissions::from_mode(0o2770)).unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o770)).unwrap();
        assert!(check_parent(&registry).is_err());
        fs::set_permissions(&config, fs::Permissions::from_mode(0o700)).unwrap();
        let alias = root.join("config-link");
        symlink(&config, &alias).unwrap();
        assert!(check_parent(&alias.join("targets.json")).is_err());
        fs::remove_file(alias).unwrap();
        fs::remove_dir(config).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn accepts_only_scoped_internal_ssh_targets() {
        for host in [
            "192.168.10.130",
            "10.1.2.3",
            "172.20.1.2",
            "100.90.0.1",
            "fd00::1",
        ] {
            assert!(validate_host(host.parse().unwrap(), 22).is_ok());
        }
        for host in [
            "127.0.0.1",
            "0.0.0.0",
            "169.254.169.254",
            "8.8.8.8",
            "::1",
            "fe80::1",
        ] {
            assert!(validate_host(host.parse().unwrap(), 22).is_err());
        }
        assert!(validate_host("10.1.2.3".parse().unwrap(), 0).is_err());
    }
    #[test]
    fn architecture_and_labels_fail_closed() {
        let mut binary = vec![0; 20];
        binary[..4].copy_from_slice(b"\x7fELF");
        binary[4] = 2;
        binary[5] = 1;
        binary[18] = 62;
        assert!(compatible_binary(&binary, "x86_64"));
        assert!(!compatible_binary(&binary, "aarch64"));
        assert!(!compatible_binary(b"fixture", "x86_64"));
        assert!(validate_label("服务器三", "").is_ok());
        assert!(validate_label("name\nsecret", "").is_err());
    }

    #[test]
    fn controller_authority_accepts_default_https_port_and_rejects_credentials_paths() {
        assert_eq!(
            controller_address("https://platform.example/")
                .ok()
                .as_deref(),
            Some("https://platform.example:443")
        );
        assert_eq!(
            controller_address("https://[fd00::1]:8443/")
                .ok()
                .as_deref(),
            Some("https://[fd00::1]:8443")
        );
        for value in [
            "http://platform:8443",
            "https://user:password@platform",
            "https://platform/other",
            "https://platform/?key=secret",
            "https://platform/#overview",
        ] {
            assert!(controller_address(value).is_err());
        }
    }
}
