//! Password/public-key onboarding and original-identity redeployment. Credentials
//! at rest use the private vault; logs never contain secrets or raw SSH output.
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
const NETWORK_CONFIG: &str = include_str!("../../deploy/linux/operations/configure-ssh-network.py");
const REDEPLOY_CHECK: &str = include_str!("../../deploy/linux/operations/check-redeployment.py");
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
    pub(super) fn running(&self,id:Uuid)->Result<bool,OperationsError>{
        Ok(self.jobs.lock().map_err(|_|OperationsError::busy())?.values().any(|job|job.node_id==id&&job.status=="running"))
    }
    fn begin(&self, id: Uuid, host: IpAddr) -> Result<Zeroizing<String>, OperationsError> {
        let key = Zeroizing::new(format!("{}{}",Uuid::new_v4().simple(),Uuid::new_v4().simple()));
        let mut jobs = self.jobs.lock().map_err(|_|OperationsError::busy())?;
        jobs.retain(|_,job|job.created.elapsed()<JOB_TTL || job.status=="running");
        if (jobs.len()>=32 && !jobs.contains_key(&id)) || jobs.values().any(|job|job.host==host && job.status=="running") {
            return Err(OperationsError::busy());
        }
        // A redeploy replaces only a completed transient job, never history or
        // a running worker. Rotating its capability invalidates old log access.
        jobs.insert(id,Job {created:Instant::now(),key_hash:crate::api::hash_token(&key),node_id:id,
            host,status:"running".into(),progress:0,events:vec![]});
        Ok(key)
    }

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
    #[serde(default = "ssh_transport::automatic")]
    connection_mode: ssh_transport::Mode,
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
        .route("/api/v1/machines/{id}/ssh", post(manage_ssh))
        .route("/api/v1/machines/{id}/redeploy", post(redeploy))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RedeployRequest {
    confirmation: String,
    username: String,
    server_url: String,
    #[serde(default="ssh_transport::automatic")]
    connection_mode: ssh_transport::Mode,
    #[serde(default)]
    use_saved: bool,
    #[serde(default)]
    auth_method: AuthMethod,
    #[serde(default,deserialize_with="deserialize_secret")]
    password: Zeroizing<String>,
    #[serde(default,deserialize_with="deserialize_secret")]
    private_key: Zeroizing<String>,
    #[serde(default,deserialize_with="deserialize_secret")]
    passphrase: Zeroizing<String>,
    #[serde(default,deserialize_with="deserialize_secret")]
    sudo_password: Zeroizing<String>,
    #[serde(default)]
    administrator: Option<Credentials>,
}

fn preserved_mode(target: &Target, observed: Option<&str>) -> DeploymentMode {
    if target.default_user=="root" || target.agent_mode.as_deref()==Some("full") || observed==Some("full") {
        DeploymentMode::Full
    } else { DeploymentMode::Basic }
}

async fn redeploy(State(state): State<OperationsState>, Path(id): Path<Uuid>, uri: Uri, headers: HeaderMap,
    payload: Result<Json<RedeployRequest>,axum::extract::rejection::JsonRejection>) -> Result<Response,OperationsError> {
    validate_origin(&headers,&uri)?;
    let Json(request) = payload.map_err(|_|OperationsError::bad_request("重新部署请求格式错误；凭据不会回显"))?;
    if request.confirmation!=id.to_string() { return Err(OperationsError::bad_request("请明确确认重新部署此原机器 ID")); }
    state.machines.rate_limit()?;
    let (target,lease) = target_and_lease(&state,id,true)?;
    let stopped=agent_service::stopped(&state.pool,id).await?;
    if target.ssh_alias.as_deref()==Some("local") && !stopped {
        return Err(OperationsError::bad_request("平台本机 Agent 受保护，请使用更新或管理员维护流程，不支持从此入口重新部署"));
    }
    if request.username!=target.default_user {
        return Err(OperationsError::bad_request("重新部署必须使用原登记 SSH 账户、地址和主机公钥，不能更换机器身份"));
    }
    let row = sqlx::query("SELECT n.deleted_at,n.last_seen,n.monitoring_paused,COALESCE(n.display_name,n.hostname) AS name,m.metrics->'privilege'->>'mode' AS mode FROM nodes n LEFT JOIN metrics_latest m ON m.node_id=n.id WHERE n.id=$1")
        .bind(id).fetch_optional(&state.pool).await.map_err(|_|OperationsError::busy())?
        .ok_or(OperationsError::bad_request("原机器不存在，不能重新部署"))?;
    let deleted: Option<DateTime<Utc>> = row.get("deleted_at");
    if deleted.is_some() { return Err(OperationsError::bad_request("请先恢复已软删除的机器，再按原 ID 重新部署")); }
    if row.get::<bool,_>("monitoring_paused") && !stopped { return Err(OperationsError::bad_request("机器监控已暂停；恢复监控即可，不应重新部署或更换身份")); }
    let last_seen: DateTime<Utc> = row.get("last_seen");
    if !stopped && last_seen>Utc::now()-chrono::Duration::seconds(state.config.online_window_seconds) {
        return Err(OperationsError::bad_request("机器仍有近期心跳；重新部署用于离线机器的覆盖修复，在线机器请使用更新或重启"));
    }
    let observed: Option<String> = row.get("mode");
    let mode = preserved_mode(&target,observed.as_deref());
    let administrator=if let Some(credentials)=request.administrator {
        if mode!=DeploymentMode::Full { return Err(OperationsError::bad_request("Basic 重新部署不接受管理员授权")); }
        Some(resolve_credentials(&state,&target,&headers,credentials)?)
    } else { None };
    let mut install = AddMachine {host:target.host,port:target.port,display_name:row.get("name"),
        server_url:controller_address(&request.server_url)?,connection_mode:request.connection_mode,
        fingerprint:String::new(),username:request.username,deployment_mode:mode,sudo_password:request.sudo_password,
        auth_method:request.auth_method,password:request.password,private_key:request.private_key,passphrase:request.passphrase};
    let mut browser_token = None;
    if request.use_saved {
        if !install.password.is_empty() || !install.private_key.is_empty() || !install.passphrase.is_empty() || !install.sudo_password.is_empty() {
            return Err(OperationsError::bad_request("使用已保存凭据时不要同时提交新凭据"));
        }
        let saved = ssh_vault::load(&state.registry,&target)?;
        if !ssh_vault::authorized(&saved,&target,&headers) {
            return Err(OperationsError(StatusCode::UNAUTHORIZED,"此浏览器尚未获准使用已保存凭据，请验证一次 SSH"));
        }
        browser_token=ssh_vault::browser_token(&saved,&target,&headers);
        install.auth_method=if saved.key.is_empty(){AuthMethod::Password}else{AuthMethod::PrivateKey};
        install.password=saved.password;install.private_key=saved.key;install.passphrase=saved.passphrase;
        if mode==DeploymentMode::Full { install.sudo_password=saved.sudo_password; }
    }
    if browser_token.is_none() && let Ok(saved)=ssh_vault::load(&state.registry,&target) {
        browser_token=ssh_vault::browser_token(&saved,&target,&headers);
    }
    install.validate_authentication()?;
    check_parent(&state.registry)?;
    candidate()?;agent_ca(&state)?;
    let permit=state.machines.permits.clone().try_acquire_owned().map_err(|_|OperationsError::busy())?;
    audit(&state.pool,id,&install.username,"machine.redeploy.request","requested").await?;
    let key=state.machines.begin(id,target.host)?;
    state.machines.event(id,0,"running","重新部署任务已创建：允许覆盖修复同一机器 ID 的安装；保留本地数据库、待上报数据、平台历史、名称和备注，保持原 Basic / Full 权限");
    launch_deployment(state,id,install,permit,&key,DeploymentContext {existing:Some(target),lease:Some(lease),browser_token,administrator})
}

/// Restore only previously enabled tunnels. Never reinstall/reconfigure remote
/// services, authenticate a soft-deleted target, or invent another node identity.
pub(super) fn restore_ssh(state: OperationsState) {
    tokio::spawn(async move {
        crate::database::wait_for_initialization(&state.initialized).await;
        let Ok(targets) = load_all_targets(&state.registry) else { return; };
        for target in targets.into_iter().filter(|t| !t.disabled && t.connection_mode==ssh_transport::Mode::Ssh) {
            let active = sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM nodes n WHERE n.id=$1 AND n.deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM agent_service_controls s WHERE s.node_id=n.id AND (s.desired_stopped OR s.state<>'running')))")
                .bind(target.node_id).fetch_one(&state.pool).await.unwrap_or(false);
            if !active { continue; }
            match ssh_vault::load(&state.registry,&target) {
                Ok(saved) if saved.enabled => {},
                Ok(_) => continue,
                Err(error) => {
                    ssh_transport::restore_status(&target,"authentication_required",0,error.1);
                    continue;
                }
            }
            let state = state.clone();
            let handle = tokio::runtime::Handle::current();
            // A sleeping restore worker must not hold Tokio runtime shutdown
            // open for two minutes when the platform itself is restarted.
            std::thread::spawn(move || {
                for attempt in 1..=ssh_transport::RETRY_LIMIT {
                    if ssh_transport::status(&target)["status"]=="connected" { return; }
                    let result = (|| {
                        // Release the GUI lease before waiting between attempts.
                        let _lease = target_and_lease(&state,target.node_id,true)?;
                        handle.block_on(agent_service::require_running(&state.pool,target.node_id))?;
                        let saved = ssh_vault::load(&state.registry,&target)?;
                        if !saved.enabled { return Err(OperationsError::bad_request("SSH 自动恢复已暂停")); }
                        let port = target.ssh_remote_port.ok_or(OperationsError::registry())?;
                        if target.server_url.as_deref()!=Some(format!("https://127.0.0.1:{port}").as_str()) {
                            return Err(OperationsError::registry());
                        }
                        let ca = agent_ca(&state)?;
                        let local = checked_local_https(&state,&handle,&ca)?;
                        let mut session = ssh_transport::onboarding(SocketAddr::new(target.host,target.port))?;
                        ssh_transport::verify_lease(&session,&target)?;
                        let auth = saved.auth();
                        auth.authenticate(&session)?;
                        ssh_transport::start(&target,auth,&mut session,local)?;
                        Ok::<_,OperationsError>(())
                    })();
                    match result {
                        Ok(()) => return,
                        Err(error) if matches!(error.0,StatusCode::UNAUTHORIZED|StatusCode::CONFLICT|StatusCode::BAD_REQUEST) => {
                            ssh_transport::restore_status(&target,"authentication_required",attempt,error.1);
                            return;
                        }
                        Err(error) => {
                            ssh_transport::restore_status(&target,if attempt==ssh_transport::RETRY_LIMIT {"retry_exhausted"}else{"reconnecting"},attempt,error.1);
                            if attempt<ssh_transport::RETRY_LIMIT { std::thread::sleep(ssh_transport::RETRY_INTERVAL); }
                        }
                    }
                }
            });
        }
    });
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SshRequest {
    action: String,
    username: String,
    #[serde(default)]
    use_saved: bool,
    #[serde(default)]
    auth_method: AuthMethod,
    #[serde(default, deserialize_with="deserialize_secret")]
    password: Zeroizing<String>,
    #[serde(default, deserialize_with="deserialize_secret")]
    private_key: Zeroizing<String>,
    #[serde(default, deserialize_with="deserialize_secret")]
    passphrase: Zeroizing<String>,
    #[serde(default, deserialize_with="deserialize_secret")]
    sudo_password: Zeroizing<String>,
}

async fn manage_ssh(State(state): State<OperationsState>, Path(id): Path<Uuid>, uri: Uri, headers: HeaderMap,
    payload: Result<Json<SshRequest>, axum::extract::rejection::JsonRejection>) -> Result<Response, OperationsError> {
    validate_origin(&headers, &uri)?;
    let Json(request) = payload.map_err(|_| OperationsError::bad_request("SSH 连接请求格式错误；鉴权内容不会回显"))?;
    if !matches!(request.action.as_str(), "connect" | "disconnect" | "forget") { return Err(OperationsError::bad_request("SSH 连接操作不正确")); }
    state.machines.rate_limit()?;
    let (mut target, lease) = target_and_lease(&state, id, true)?;
    if request.action=="connect" { agent_service::require_running(&state.pool,id).await?; }
    if request.username != target.default_user { return Err(OperationsError::bad_request("请使用首次部署登记的 SSH 账户重连；不会更换机器身份")); }
    if state.machines.jobs.lock().map_err(|_| OperationsError::busy())?.values().any(|job| job.node_id==id && job.status=="running") {
        return Err(OperationsError::busy());
    }
    let observed_mode: Option<String> = sqlx::query_scalar("SELECT (SELECT metrics->'privilege'->>'mode' FROM metrics_latest WHERE node_id=$1)")
        .bind(id).fetch_one(&state.pool).await.map_err(|_| OperationsError::busy())?;
    let full = observed_mode.as_deref().or(target.agent_mode.as_deref())==Some("full");
    let saved = if request.use_saved {
        if !request.password.is_empty() || !request.private_key.is_empty() || !request.passphrase.is_empty() || !request.sudo_password.is_empty() {
            return Err(OperationsError::bad_request("使用已保存凭据时不要同时发送新凭据"));
        }
        let saved = ssh_vault::load(&state.registry, &target)?;
        if !ssh_vault::authorized(&saved, &target, &headers) {
            return Err(OperationsError(StatusCode::UNAUTHORIZED,"此浏览器尚未获准使用已保存凭据，请验证一次 SSH；不会返回保存的密码或私钥"));
        }
        Some(saved)
    } else { None };
    let action = request.action;
    let mut authentication = AddMachine { host: target.host, port: target.port, display_name: target.hostname.clone(),
        server_url: target.server_url.clone().unwrap_or_default(), connection_mode: ssh_transport::Mode::Ssh,
        fingerprint: String::new(), username: request.username, deployment_mode: if full { DeploymentMode::Full } else { DeploymentMode::Basic },
        sudo_password: request.sudo_password, auth_method: request.auth_method, password: request.password,
        private_key: request.private_key, passphrase: request.passphrase };
    if let Some(saved) = saved {
        authentication.auth_method = if saved.key.is_empty() { AuthMethod::Password } else { AuthMethod::PrivateKey };
        authentication.password = saved.password;
        authentication.private_key = saved.key;
        authentication.passphrase = saved.passphrase;
        authentication.sudo_password = if full { saved.sudo_password } else { Zeroizing::new(String::new()) };
    }
    authentication.validate_authentication()?;
    audit(&state.pool, id, &authentication.username, &format!("machine.ssh.{action}.request"), "requested").await?;
    let actor = authentication.username.clone();
    let worker_state = state.clone();
    let handle = tokio::runtime::Handle::current();
    let disconnect = action != "connect";
    let forget = action == "forget";
    let use_saved = request.use_saved;
    let browser_cookie = Arc::new(Mutex::new(None::<String>));
    let worker_cookie = browser_cookie.clone();
    let worker = tokio::task::spawn_blocking(move || {
        let _lease = lease;
        let auth = ssh_auth(&authentication);
        if disconnect {
            if !use_saved {
                let mut direct = target.clone();
                direct.connection_mode = ssh_transport::Mode::Direct;
                let _session = ssh_transport::authenticated(&direct, &auth, &Cancellation::default())?;
            }
            ssh_transport::stop(&target)?;
            if ssh_vault::exists(&worker_state.registry, &target) {
                if forget { ssh_vault::forget(&worker_state.registry, &target)?; }
                else { ssh_vault::set_enabled(&worker_state.registry, &target, false)?; }
            }
            return Ok::<_,OperationsError>(());
        }
        // Prove a fresh/manual credential before allowing it to stop a live tunnel.
        if !use_saved && matches!(ssh_transport::status(&target)["status"].as_str(),Some("connected"|"reconnecting")) {
            let _proof = ssh_transport::authenticated(&target,&auth,&Cancellation::default())?;
        }
        // Explicit reconnect stops the old worker before reusing its slot budget.
        ssh_transport::stop(&target)?;
        let mut session = ssh_transport::onboarding(SocketAddr::new(target.host, target.port))?;
        ssh_transport::verify_lease(&session, &target)?;
        if full && authentication.username!="root" && authentication.sudo_password.is_empty() && authentication.auth_method==AuthMethod::Password {
            authentication.sudo_password = authentication.password.clone();
        }
        authenticate(&session, &mut authentication)?;
        if !use_saved {
            let token = ssh_vault::save(&worker_state.registry, &target, &auth, &authentication.sudo_password, false)?;
            *worker_cookie.lock().map_err(|_| OperationsError::busy())? = Some(ssh_vault::cookie(&target, &token));
        }
        let ca = agent_ca(&worker_state)?;
        let local = checked_local_https(&worker_state, &handle, &ca)?;
        let port = ssh_transport::start(&target, auth, &mut session, local)?;
        let mut setup = ssh_transport::SetupGuard::new(&target);
        // Renew only an unregistered node's scoped credential, never a global
        // enrollment secret or an already registered Agent identity/token.
        let token = Zeroizing::new(format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()));
        let renewal = handle.block_on(async {
            sqlx::query("INSERT INTO machine_enrollments(node_id,token_hash,expires_at,expected_user) SELECT $1,$2,NOW()+INTERVAL '10 minutes',$3 WHERE NOT EXISTS(SELECT 1 FROM agents WHERE node_id=$1) ON CONFLICT(node_id) DO UPDATE SET token_hash=EXCLUDED.token_hash,expires_at=EXCLUDED.expires_at,expected_user=EXCLUDED.expected_user")
                .bind(id).bind(crate::api::hash_token(&token)).bind(&target.default_user).execute(&worker_state.pool).await
        }).map_err(|_| OperationsError::busy())?.rows_affected()>0;
        let url = format!("https://127.0.0.1:{port}");
        let result = configure_network(&session, &authentication, &target, &url, renewal.then_some(token.as_str()));
        if let Err(error) = result { let _ = ssh_transport::stop(&target); return Err(error); }
        target.connection_mode = ssh_transport::Mode::Ssh;
        target.ssh_remote_port = Some(port);
        target.server_url = Some(url);
        save_target(&worker_state, &target)?;
        if full {
            // Same authenticated control connection; no new SSH invocation.
            admin_command(&session, &authentication, "/usr/bin/systemctl restart easymanage-agent.service")?;
        } else { restart_basic_on(&session, &target)?; }
        ssh_vault::set_enabled(&worker_state.registry, &target, true)?;
        setup.commit();
        Ok(())
    });
    let result = match tokio::time::timeout(Duration::from_secs(100), worker).await {
        Ok(Ok(result)) => result, _ => Err(OperationsError::ssh("SSH 网络切换超时；请检查连接状态和目标私有日志，不推断已回滚"))
    };
    let _ = audit(&state.pool, id, &actor, &format!("machine.ssh.{action}.result"), if result.is_ok(){"success"}else{"failed"}).await;
    let mut response = match result {
        Ok(()) => Json(json!({"message":if forget {"SSH 长连接已断开并删除平台保存的凭据；远端安装和机器身份保留"}
            else if disconnect {"SSH 长连接已断开并暂停自动恢复；加密凭据保留，可一键重连"}
            else {"SSH 长连接已建立，凭据已加密保存；请以真实新心跳确认接入成功"}})).into_response(),
        Err(error) => error.into_response(),
    };
    if let Some(cookie) = browser_cookie.lock().map_err(|_| OperationsError::busy())?.take() {
        response.headers_mut().insert(axum::http::header::SET_COOKIE, cookie.parse().map_err(|_| OperationsError::busy())?);
    }
    Ok(response)
}

fn configure_network(session: &Session, request: &AddMachine, target: &Target, url: &str, token: Option<&str>) -> Result<(), OperationsError> {
    let invocation = format!("/usr/bin/python3 -I -c {} {} {} {} {} {}", shell_quote(NETWORK_CONFIG),
        shell_quote(&target.agent_home.to_string_lossy()), shell_quote(&target.node_id.to_string()), shell_quote(url),
        shell_quote(target.ca_sha256.as_deref().ok_or(OperationsError::registry())?), shell_quote(request.deployment_mode.name()));
    let invocation = if request.deployment_mode==DeploymentMode::Full && request.username!="root" { format!("/usr/bin/sudo -k -S -p '' {invocation}") } else { invocation };
    let mut channel = crate::network::ssh_channel(&session).map_err(|_| OperationsError::ssh("网络配置通道不可用"))?;
    channel.handle_extended_data(ExtendedData::Ignore).map_err(|_| OperationsError::busy())?;
    channel.exec(&invocation).map_err(|_| OperationsError::ssh("网络配置启动失败"))?;
    if request.deployment_mode==DeploymentMode::Full && request.username!="root" {
        channel.write_all(request.sudo_password.as_bytes()).and_then(|()|channel.write_all(b"\n")).map_err(|_|OperationsError::ssh("管理员认证提交失败"))?;
    }
    let payload = Zeroizing::new(serde_json::to_vec(&json!({"enrollment_token":token})).map_err(|_|OperationsError::busy())?);
    channel.write_all(&payload).map_err(|_|OperationsError::ssh("网络配置传输失败"))?;
    channel.send_eof().map_err(|_|OperationsError::ssh("网络配置传输失败"))?;
    let mut discarded = Zeroizing::new(Vec::new());
    (&mut channel).take(8193).read_to_end(&mut discarded).map_err(|_|OperationsError::ssh("网络配置读取失败"))?;
    if discarded.len()>8192 { return Err(OperationsError::ssh("网络配置输出超过上限")); }
    channel.wait_close().map_err(|_|OperationsError::ssh("网络配置通道中断"))?;
    let reason = match channel.exit_status().unwrap_or(-1) {
        0 => return Ok(()),
        20 => "未找到原 Agent 配置；可能已卸载或从未完成安装，请使用原机器的「重新部署」，不能仅切换网络或生成新机器 ID",
        21 => "原 Agent 配置/CA 的所有者、权限或路径不符合要求；未修改文件，请检查原 SSH 账户与私有目录",
        22 => "远端 Agent CA 与平台登记摘要不同；未修改文件，请核对原 CA，不会跳过 TLS 校验",
        23 => "远端 Agent 身份与原机器 ID 不一致或无法确认；未修改配置，也未重置 SQLite",
        24 => "实际服务权限与所选模式不同；已有 Full 必须使用原 root/sudo 配置，不会创建 Basic 替代服务",
        25 => "原 Agent 配置格式不兼容或有重复项；未修改文件，请检查 agent.env",
        26 => "写入原 Agent 私有配置失败；请检查剩余磁盘空间、目录写权限及保留的恢复文件",
        _ => "网络配置通道失败；请检查 Python3、root/sudo 授权和原安装；不会输出远端原始内容或凭据",
    };
    return Err(OperationsError::ssh(reason));
}

pub(super) fn restart_basic_on(session: &Session, target: &Target) -> Result<(), OperationsError> {
    let script = format!("set -eu; if command -v systemctl >/dev/null; then if systemctl is-active --quiet easymanage-agent.service || systemctl is-enabled --quiet easymanage-agent.service; then exit 3; fi; fi; base={}; if test -f \"$base/config/agent-launch.json\" && python3 -c 'import json,sys;sys.exit(0 if json.load(open(sys.argv[1])).get(\"backend\")==\"compatibility\" else 1)' \"$base/config/agent-launch.json\"; then python3 \"$base/bin/agent-user-control.py\" restart; else systemctl --user restart easymanage-agent.service; fi", shell_quote(&target.agent_home.to_string_lossy()));
    command(session, &format!("/usr/bin/timeout --signal=TERM --kill-after=2s 40s /bin/bash --noprofile --norc -c {}", shell_quote(&script))).map(|_|())
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
        ssh_transport::probe(SocketAddr::new(request.host, request.port))
    })
    .await
    .map_err(|_| OperationsError::busy())??;
    Ok(Json(
        json!({"fingerprint":value, "message":"尚未发送密码。请与目标机 SSH 主机指纹核对，确认后才开始部署。"}),
    ))
}

async fn list(
    State(state): State<OperationsState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, OperationsError> {
    let targets = load_all_targets(&state.registry)?;
    let platform_admin_available = platform_target(&state).is_ok();
    let rows = sqlx::query("SELECT n.id,n.hostname,n.display_name,n.management_note,n.deleted_at,n.last_seen,n.monitoring_paused,n.monitoring_revision,n.monitoring_applied_revision,n.control_seen_at,s.state AS agent_service_state,s.revision AS agent_service_revision,s.desired_stopped AS agent_service_desired_stopped,s.last_error AS agent_service_last_error,m.metrics->'agent_control' AS agent_control,m.metrics->'privilege'->>'mode' AS mode FROM nodes n LEFT JOIN metrics_latest m ON m.node_id=n.id LEFT JOIN agent_service_controls s ON s.node_id=n.id ORDER BY n.hostname")
        .fetch_all(&state.pool).await.map_err(OperationsError::database)?;
    let machines: Vec<_> = rows.iter().map(|row| {
        let id: Uuid = row.get("id");
        let target = targets.iter().find(|target| target.node_id == id);
        let observed: Option<String> = row.get("mode");
        let last_seen: DateTime<Utc> = row.get("last_seen");
        let deleted: Option<DateTime<Utc>> = row.get("deleted_at");
        let service_state: Option<String>=row.get("agent_service_state");
        let stopped=service_state.as_deref().is_some_and(|value|value!="running");
        json!({"node_id":id,"hostname":row.get::<String,_>("hostname"),
            "agent_service_state":service_state,"agent_service_revision":row.get::<Option<i64>,_>("agent_service_revision").unwrap_or(0),
            "agent_service_desired_stopped":row.get::<Option<bool>,_>("agent_service_desired_stopped").unwrap_or(false),
            "agent_service_last_error":row.get::<Option<String>,_>("agent_service_last_error"),
            "agent_stop_available":target.is_some(),
            "monitoring_paused":row.get::<bool,_>("monitoring_paused"),
            "monitoring_revision":row.get::<i64,_>("monitoring_revision"),
            "monitoring_applied_revision":row.get::<Option<i64>,_>("monitoring_applied_revision"),
            "control_seen_at":row.get::<Option<DateTime<Utc>>,_>("control_seen_at"),
            "monitoring_supported":row.get::<Option<serde_json::Value>,_>("agent_control").is_some_and(|v|!v.is_null()) || row.get::<Option<i64>,_>("monitoring_applied_revision").is_some(),
            "monitoring_reported_revision":row.get::<Option<serde_json::Value>,_>("agent_control").and_then(|v|v.get("revision").and_then(serde_json::Value::as_u64)),
            "display_name":row.get::<Option<String>,_>("display_name"),
            "note":row.get::<String,_>("management_note"),"deleted_at":row.get::<Option<DateTime<Utc>>,_>("deleted_at"),
            "last_seen":row.get::<DateTime<Utc>,_>("last_seen"),
            "host":target.map(|t|t.host),"port":target.map(|t|t.port),
            "username":target.map(|t|t.default_user.clone()),"manageable":target.is_some() || platform_admin_available,
            "connection_mode":target.map(|t|t.connection_mode),
            "credentials_saved":target.is_some_and(|t|ssh_vault::exists(&state.registry,t)),
            "deployment_mode":target.map(|t|preserved_mode(t,observed.as_deref()).name()),
            "redeploy_available":deleted.is_none() && (stopped || (!row.get::<bool,_>("monitoring_paused") && last_seen<=Utc::now()-chrono::Duration::seconds(state.config.online_window_seconds)))
                && target.is_some_and(|t|stopped || t.ssh_alias.as_deref()!=Some("local")),
            "saved_credentials_usable":target.is_some_and(|t|ssh_vault::load(&state.registry,t).is_ok_and(|s|ssh_vault::authorized(&s,t,&headers))),
            "ssh_transport":target.filter(|t|t.connection_mode==ssh_transport::Mode::Ssh).map(ssh_transport::status)})
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
) -> Result<Response, OperationsError> {
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
    let key=state.machines.begin(id,request.host)?;
    let deployment_mode = request.deployment_mode;
    state.machines.event(
        id,
        0,
        "running",
        if deployment_mode == DeploymentMode::Basic { "任务已创建；普通用户模式，所有安装文件位于 SSH 用户 home，不执行 sudo" }
        else { "任务已创建；已选择首次安装管理员 Full Agent，将验证 root/sudo 权限" },
    );
    launch_deployment(state,id,request,permit,&key,DeploymentContext::default())
}

#[derive(Default)]
struct DeploymentContext {
    existing: Option<Target>,
    lease: Option<Lease>,
    browser_token: Option<Zeroizing<String>>,
    administrator: Option<Credentials>,
}

fn launch_deployment(state: OperationsState,id: Uuid,request: AddMachine,
    permit: tokio::sync::OwnedSemaphorePermit,key: &str,context: DeploymentContext) -> Result<Response,OperationsError> {
    let response = json!({"job_id":id,"job_key":key,"node_id":id});
    let browser_token = context.browser_token.unwrap_or_else(||Zeroizing::new(format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())));
    let cookie = ssh_vault::cookie_for(id,&browser_token);
    let actor = request.username.clone();
    let deployment_mode = request.deployment_mode;
    let redeploy = context.existing.is_some();
    tokio::spawn(async move {
        let _permit = permit;
        let _lease = context.lease;
        let worker_state = state.clone();
        let handle = tokio::runtime::Handle::current();
        let worker =
            tokio::task::spawn_blocking(move || deploy(&worker_state, id, request, &handle, &browser_token,context.existing,context.administrator));
        let result = worker.await;
        let outcome = if matches!(&result, Ok(Ok(()))) {
            "success"
        } else {
            "failed"
        };
        if redeploy && agent_service::redeploy_finish(&state,id,outcome=="success").await.is_err() {
            state.machines.event(id,100,"failed","远端部署结果已返回，但平台服务状态未能保存；请刷新核验，不推断已成功");
            return;
        }
        if audit(&state.pool, id, &actor, if redeploy {"machine.redeploy.result"}else{"machine.add.result"}, outcome)
            .await
            .is_err()
        {
            tracing::error!(%id,"machine deployment result audit unavailable");
        }
        match result {
            Ok(Ok(())) => {
                state
                    .machines
                    .event(id, 100, "success", if redeploy {"已收到重新部署后新进程的真实心跳；原机器 ID、本地数据库、待上报数据和平台历史保留，连接凭据已更新"}
                        else if deployment_mode == DeploymentMode::Basic { "已收到新 Agent 的 Basic 心跳，部署完成；后续可通过设备总览提权" } else { "已收到新 Agent 的 Full / UID 0 心跳，管理员部署完成" })
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
    let mut response = Json(response).into_response();
    response.headers_mut().insert(axum::http::header::SET_COOKIE,cookie.parse().map_err(|_|OperationsError::busy())?);
    Ok(response)
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

pub(super) fn command(session: &Session, invocation: &str) -> Result<String, OperationsError> {
    let mut channel = crate::network::ssh_channel(session)
        .map_err(|_| OperationsError::ssh("SSH 通道不可用"))?;
    channel.handle_extended_data(ExtendedData::Merge).map_err(|_| OperationsError::ssh("SSH 通道初始化失败"))?;
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

pub(super) fn check_parent(registry: &FsPath) -> Result<(), OperationsError> {
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

pub(super) fn save_target(state: &OperationsState, target: &Target) -> Result<(), OperationsError> {
    let _lock = state
        .machines
        .writes
        .lock()
        .map_err(|_| OperationsError::busy())?;
    check_parent(&state.registry)?;
    let lock_path=state.registry.with_file_name("managed-targets.lock");
    if lock_path.exists() || lock_path.is_symlink() { check_private_file(&lock_path)?; }
    let file_lock=OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600)
        .open(&lock_path).map_err(|_|OperationsError::registry())?;
    file_lock.try_lock().map_err(|_|OperationsError::busy())?;
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
    let mut stream = crate::network::scp_send(session,path, mode, bytes.len() as u64)
        .map_err(|_| OperationsError::ssh("上传暂存文件失败"))?;
    stream
        .write_all(bytes)
        .map_err(|_| OperationsError::ssh("文件上传中断"))?;
    // SCP sink requires an explicit file terminator and an ACK before EOF.
    // A file existing remotely is not proof that the sink accepted the transfer.
    stream.write_all(b"\0").map_err(|_| OperationsError::ssh("上传结束标记发送失败"))?;
    let mut ack=[0_u8;1];
    stream.read_exact(&mut ack).map_err(|_| OperationsError::ssh("上传确认中断"))?;
    if ack != [0] { return Err(OperationsError::ssh("目标未确认文件上传；未继续安装")); }
    stream
        .send_eof()
        .and_then(|()| stream.wait_eof())
        .and_then(|()| stream.close())
        .and_then(|()| stream.wait_close())
        .map_err(|_| OperationsError::ssh("上传校验通道中断"))?;
    if stream.exit_status().unwrap_or(-1)!=0 { return Err(OperationsError::ssh("文件上传进程退出异常；未继续安装")); }
    Ok(())
}

pub(super) fn update_over_ssh(target: &Target, binary: &FsPath, full: bool) -> Result<Option<String>, OperationsError> {
    let session = ssh_transport::primary(target)?;
    session.set_timeout(100_000);
    let base = target.agent_home.to_string_lossy();
    let stage = command(&session, &format!("umask 077; test ! -L {base}; test ! -L {staging}; mkdir -p {staging}; mktemp -d {template}",
        base=shell_quote(&base), staging=shell_quote(&format!("{base}/staging")), template=shell_quote(&format!("{base}/staging/agent.XXXXXXXX"))))?.trim().to_owned();
    let prefix = format!("{base}/staging/agent.");
    if !stage.starts_with(&prefix) || stage[prefix.len()..].len()!=8 || !stage[prefix.len()..].bytes().all(|b|b.is_ascii_alphanumeric()) {
        return Err(OperationsError::ssh("SSH 暂存目录不安全"));
    }
    let binary = fs::read(binary).map_err(|_|OperationsError::registry())?;
    let observed_hash=hex::encode(Sha256::digest(&binary));
    if target.release_sha256.as_deref()!=Some(observed_hash.as_str()) {
        return Err(OperationsError::bad_request("候选在上传前已改变；请刷新版本后重试，不会安装未确认的文件"));
    }
    let mut files:Vec<(&str,Vec<u8>,i32)> = vec![("easymanage-agent",binary,0o700)];
    if !full {
        files.extend([
            ("install-user-agent.sh",USER_INSTALL.as_bytes().to_vec(),0o700),
            ("agent-user-control.py",USER_CONTROL.as_bytes().to_vec(),0o700),
            ("easymanage-agent.user.service",USER_UNIT.as_bytes().to_vec(),0o600),
            ("node.json",serde_json::to_vec(&json!({"node_id":target.node_id,"user":target.default_user,"agent_home":target.agent_home})).map_err(|_|OperationsError::registry())?,0o600),
        ]);
    }
    let manifest = files.iter().map(|(name,bytes,_)|format!("{}  {name}\n",hex::encode(Sha256::digest(bytes)))).collect::<String>();
    for (name,bytes,mode) in &files { upload(&session,&PathBuf::from(&stage).join(name),bytes,*mode)?; }
    upload(&session,&PathBuf::from(&stage).join("SHA256SUMS"),manifest.as_bytes(),0o600)?;
    if !full {
        // Existing private env/CA stay on the target. No SSH credentials or
        // Agent registration tokens are pulled back into command output.
        let script = format!("set -eu; cd {}; cp -- {} agent.env; cp -- {} ca.crt; sha256sum agent.env ca.crt >> SHA256SUMS; /bin/bash ./install-user-agent.sh",
            shell_quote(&stage),shell_quote(&format!("{base}/config/agent.env")),shell_quote(&format!("{base}/config/ca.crt")));
        let marker=format!("EASYMANAGE_UPDATE_{}:",Uuid::new_v4().simple());
        let invocation=format!("/bin/bash --noprofile --norc -c {}; result=$?; if test \"$result\" -ne 0; then printf '\\n{}FAIL\\n'; exit \"$result\"; fi; digest=$(sha256sum -- {}); digest=${{digest%% *}}; printf '\\n{}%s\\n' \"$digest\"",
            shell_quote(&script),marker,shell_quote(&format!("{base}/bin/easymanage-agent")),marker);
        command_with_receipt(&session,&invocation,&marker,&observed_hash)?;
    }
    // Keep the exact staged bundle for diagnostics/recovery. No recursive cleanup.
    Ok(full.then_some(stage))
}

fn update_receipt(output:&[u8],marker:&str,expected:&str)->Option<Result<(),OperationsError>>{
    for line in output.split_inclusive(|byte|*byte==b'\n') {
        if line.last()!=Some(&b'\n') {continue;}
        if let Some(digest)=line.strip_suffix(b"\n").and_then(|line|line.strip_prefix(marker.as_bytes())) {
            return Some(if crate::api::constant_time_equal(std::str::from_utf8(digest).unwrap_or(""),expected) {Ok(())}
                else {Err(OperationsError::ssh("更新完成回执的摘要不匹配或安装失败；保留原身份及恢复文件"))});
        }
    }
    None
}

fn command_with_receipt(session:&Session,invocation:&str,marker:&str,expected:&str)->Result<(),OperationsError>{
    let mut channel=crate::network::ssh_channel(session).map_err(|_|OperationsError::ssh("SSH 更新通道不可用"))?;
    channel.handle_extended_data(ExtendedData::Merge).map_err(|_|OperationsError::ssh("SSH 更新通道初始化失败"))?;
    channel.exec(invocation).map_err(|_|OperationsError::ssh("SSH 更新启动失败"))?;
    let mut output=Vec::new();let mut buffer=[0_u8;1024];
    loop {
        let length=channel.read(&mut buffer).map_err(|_|OperationsError::ssh("更新回执超时或中断；请核对节点心跳及保留的恢复文件"))?;
        if length==0 {return Err(OperationsError::ssh("更新未返回已校验的完成回执；请核对节点心跳"));}
        output.extend_from_slice(&buffer[..length]);
        if output.len()>8192 {return Err(OperationsError::ssh("更新回执超过输出上限"));}
        if let Some(result)=update_receipt(&output,marker,expected) {
            result?;
            channel.confirm_completion();
            // Do not wait for EOF held by an unrelated detached descendant.
            // The lease restores the ordinary timeout on return.
            session.set_timeout(1000);let _=channel.close();return Ok(());
        }
    }
}

#[cfg(test)]
mod update_receipt_tests {
    use super::*;
    #[test]
    fn only_complete_nonce_and_hash_verified_receipts_finish_an_update(){
        let marker="EASYMANAGE_UPDATE_nonce:";let digest="a".repeat(64);
        assert!(update_receipt(b"installation output\n",marker,&digest).is_none());
        assert!(update_receipt(format!("{marker}{digest}").as_bytes(),marker,&digest).is_none());
        assert!(update_receipt(format!("other:{digest}\n").as_bytes(),marker,&digest).is_none());
        assert!(update_receipt(format!("status\n{marker}{digest}\n").as_bytes(),marker,&digest).unwrap().is_ok());
        assert!(update_receipt(format!("{marker}FAIL\n").as_bytes(),marker,&digest).unwrap().is_err());
        assert!(update_receipt(format!("{marker}{}\n","b".repeat(64)).as_bytes(),marker,&digest).unwrap().is_err());
    }
}

pub(super) fn finish_full_update(target: &Target, credentials: Credentials, stage: &str, hash: &str, cancellation: &Cancellation) -> Result<(), OperationsError> {
    let invocation = format!("/usr/bin/timeout --signal=TERM --kill-after=3s 50s /usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin /bin/bash --noprofile --norc -c {} -- {} {} {} {}",
        shell_quote(include_str!("../../deploy/linux/operations/update-full-over-ssh.sh")),shell_quote(stage),shell_quote(&target.node_id.to_string()),shell_quote(hash),shell_quote(target.ca_sha256.as_deref().ok_or(OperationsError::registry())?));
    run_privileged_command(target,credentials,cancellation,&invocation)
}

fn deployment_home(username: &str) -> PathBuf {
    if username == "root" { PathBuf::from("/root") } else { PathBuf::from(format!("/home/{username}")) }
}

fn ssh_auth(request: &AddMachine) -> ssh_transport::Auth {
    ssh_transport::Auth { username: request.username.clone(), password: request.password.clone(),
        key: request.private_key.clone(), passphrase: request.passphrase.clone() }
}

fn direct_reachable(session: &Session, url: &str, ca: &[u8]) -> Result<bool, OperationsError> {
    // No credentials, no response body, no raw exception text. The public CA is
    // passed as data; no curl/Python proxy environment can redirect the probe.
    let script = "import base64,ssl,sys,urllib.request\ntry:\n c=ssl.create_default_context(cadata=base64.b64decode(sys.argv[2]).decode());o=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=c))\n with o.open(sys.argv[1]+'/api/v1/health/ready',timeout=3) as r: print('direct' if r.status==200 else 'ssh')\nexcept Exception: print('ssh')";
    let invocation = format!("python3 -c {} {} {}", shell_quote(script), shell_quote(url),
        shell_quote(&base64::engine::general_purpose::STANDARD.encode(ca)));
    Ok(command(session, &invocation)?.trim() == "direct")
}

fn checked_local_https(state: &OperationsState, handle: &tokio::runtime::Handle, ca: &[u8]) -> Result<SocketAddr, OperationsError> {
    // The Agent validates this loopback IP through the tunnel as well. Never
    // disable certificate/hostname checks or copy the platform private key.
    let local = SocketAddr::from(([127,0,0,1], state.config.bind_address.port()));
    let certificate = reqwest::Certificate::from_pem(ca).map_err(|_| OperationsError::registry())?;
    let client = reqwest::Client::builder().add_root_certificate(certificate).no_proxy()
        .redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(5)).build()
        .map_err(|_| OperationsError::registry())?;
    let ready = handle.block_on(async { crate::network::service_request(client.get(format!("https://{local}/api/v1/health/ready")),easymanage_protocol::NetworkGroup::HttpProbe).await
        .is_ok_and(|response| response.status.is_success()) });
    if !ready { return Err(OperationsError::ssh("SSH 上报需要平台回环 HTTPS 可达且证书包含 127.0.0.1；请检查 CA/证书，不会跳过校验")); }
    Ok(local)
}

// Only fixed controller-owned scripts are executed as root. Secrets go over
// channel stdin, never command arguments, remote files or job logs.
fn admin_command(session: &Session, request: &AddMachine, invocation: &str) -> Result<(), OperationsError> {
    let invocation = if request.username == "root" { invocation.to_owned() }
        else { format!("/usr/bin/sudo -k -S -p '' {invocation}") };
    let mut channel = crate::network::ssh_channel(session).map_err(|_| OperationsError::ssh("管理员通道不可用"))?;
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
    browser_token: &str,
    existing: Option<Target>,
    administrator: Option<Credentials>,
) -> Result<(), OperationsError> {
    state.machines.event(
        id,
        5,
        "running",
        "连接 SSH 并核验已确认的主机指纹（尚未进行身份鉴权）",
    );
    let ssh_auth = ssh_auth(&request);
    let pooled = existing.as_ref().is_some_and(|target|ssh_transport::status(target)["status"]=="connected");
    let mut session = if pooled {
        ssh_transport::authenticated(existing.as_ref().ok_or(OperationsError::registry())?,&ssh_auth,&Cancellation::default())?
    } else {
        if let Some(target) = existing.as_ref() { ssh_transport::stop(target)?; }
        ssh_transport::onboarding(SocketAddr::new(request.host, request.port))?
    };
    if let Some(target) = existing.as_ref() {
        // Pinned original host key is checked BEFORE any password/key is sent.
        ssh_transport::verify_lease(&session,target)?;
    } else if fingerprint(&session)? != request.fingerprint {
        return Err(OperationsError::ssh(
            "SSH 指纹与确认值不同；未进行身份鉴权，已停止部署",
        ));
    }
    let mode = request.deployment_mode;
    if mode == DeploymentMode::Full && request.username != "root" && request.sudo_password.is_empty()
        && request.auth_method == AuthMethod::Password {
        request.sudo_password = Zeroizing::new(request.password.to_string());
    }
    if session.authenticated() { request.clear_ssh_credentials(); }
    else { authenticate(&session, &mut request)?; }
    let same_admin=administrator.as_ref().is_some_and(|credentials|credentials.username==ssh_auth.username
        && crate::api::constant_time_equal(&credentials.password,&ssh_auth.password)
        && crate::api::constant_time_equal(&credentials.private_key,&ssh_auth.key)
        && crate::api::constant_time_equal(&credentials.passphrase,&ssh_auth.passphrase));
    let admin_session=if let Some(credentials)=administrator.as_ref().filter(|_|!same_admin) {
        let mut target=existing.clone().ok_or(OperationsError::registry())?;
        if !pooled { target.connection_mode=ssh_transport::Mode::Direct; }
        Some(ssh_connect(&target,credentials,&Cancellation::default())?)
    } else { None };
    if let Some(target) = existing.as_ref() {
        let enabled=ssh_vault::load(&state.registry,target).is_ok_and(|saved|saved.enabled);
        ssh_vault::save_with_token(&state.registry,target,&ssh_auth,&request.sudo_password,enabled,browser_token)?;
    }
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
        if let Some(credentials)=administrator.as_ref() { run_privileged_on(admin_session.as_deref().unwrap_or(&session),credentials.clone(),"/usr/bin/true")?; }
        else { admin_command(&session, &request, "/usr/bin/true")?; }
    }
    let binary_path = candidate()?;
    let binary = fs::read(&binary_path).map_err(|_| OperationsError::registry())?;
    let hash = hex::encode(Sha256::digest(&binary));
    if !compatible_binary(&binary, fields[2]) {
        return Err(OperationsError::bad_request(
            "候选 Agent 与目标 CPU 架构不匹配；未安装任何文件",
        ));
    }
    // New onboarding never reassigns an existing UUID. Explicit original-node
    // repair may replace files, but only after checking all retained identity data.
    command(&session, "test ! -e \"$HOME/.local/share/easymanage/config/agent.env\" && test ! -e \"$HOME/.local/share/easymanage/state/state.db\"")
        .map_err(|_| OperationsError::bad_request("检测到旧用户目录 Agent 身份，请先迁移到 ~/.easymanage，不允许重复注册"))?;
    if existing.is_none() {
    command(&session,"/bin/bash --noprofile --norc -c 'set -eu; if command -v systemctl >/dev/null; then state=$(systemctl show easymanage-agent.service -p LoadState --value 2>/dev/null || true); test -z \"$state\" || test \"$state\" = not-found; fi; test ! -e /etc/systemd/system/easymanage-agent.service; test ! -d /var/lib/easymanage-agent; test ! -e /usr/local/sbin/easymanage-agent; test ! -d /etc/easymanage || test -x /etc/easymanage; test ! -e /etc/easymanage/agent.env; test ! -e \"$HOME/.easymanage/state/state.db\"; test ! -e \"$HOME/.easymanage/config/agent.env\"; test ! -e \"$HOME/.config/systemd/user/easymanage-agent.service\"' ")
        .map_err(|_|OperationsError::bad_request("目标已存在 Agent 配置、用户单元或身份库；请恢复/接管原节点，不允许覆盖安装"))?;
    } else {
        let check=format!("/usr/bin/python3 -c {} {} {} {}",shell_quote(REDEPLOY_CHECK),
            shell_quote(&format!("{}/.easymanage",fields[1])),shell_quote(&id.to_string()),mode.name());
        let checked=if let Some(credentials)=administrator.as_ref() {run_privileged_on(admin_session.as_deref().unwrap_or(&session),credentials.clone(),&check)}
            else if mode==DeploymentMode::Full {admin_command(&session,&request,&check)} else {command(&session,&check).map(|_|())};
        checked.map_err(|_|OperationsError::bad_request("已有安装的机器 ID、数据库路径或权限与原节点不符；未覆盖任何文件，请核对原身份，Basic 不会覆盖 Full 服务"))?;
        state.machines.event(id,12,"running","已核验已有安装属于原机器 ID；将备份程序/配置并保留 SQLite 身份、待上报数据和采集设置，仅重启本 Agent");
        handle.block_on(agent_service::redeploy_begin(&state.pool,id))?;
    }
    let bootstrap = Zeroizing::new(format!(
        "{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    ));
    state.machines.event(
        id,
        15,
        "running",
        if existing.is_some() {"已核验原机器 ID 与 SSH 主机公钥；准备重新部署，不新建节点"}
        else {"已为新机器分配待注册 ID；准备 Agent CA 与已核验的 SSH 公钥"},
    );
    let home = PathBuf::from(fields[1]).join(".easymanage");
    // Deploy the Agent's trusted CA, not the platform leaf certificate.
    let ca = agent_ca(state)?;
    let key_path = state
        .registry
        .with_file_name(format!("machine-{id}.known_hosts"));
    check_parent(&state.registry)?;
    if existing.is_none() {
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
    }
    let mut target = existing.clone().unwrap_or_else(||Target {
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
        release_sha256: Some(hash.clone()),
        ssh_alias: None,
        disabled: false,
        connection_mode: ssh_transport::Mode::Direct,
        ssh_remote_port: None,
        agent_mode: Some(mode.name().to_owned()),
    });
    target.hostname=request.display_name.clone();
    if existing.is_none() { target.agent_sha256=None; }
    target.server_url=Some(request.server_url.clone());
    target.ca_sha256=Some(hex::encode(Sha256::digest(&ca)));
    target.release_sha256=Some(hash);
    target.agent_mode=Some(mode.name().to_owned());
    let mut setup = None;
    let use_ssh = request.connection_mode == ssh_transport::Mode::Ssh ||
        (request.connection_mode == ssh_transport::Mode::Auto && !direct_reachable(&session, &request.server_url, &ca)?);
    if use_ssh {
        state.machines.event(id, 23, "running", "目标无法直连或已选择 SSH：建立长期控制与上报连接，每目标最多 3 条 SSH，不新增公网监听");
        let port = if pooled {
            target.ssh_remote_port.ok_or(OperationsError::registry())?
        } else {
            target.ssh_remote_port=None;
            let local = checked_local_https(state, handle, &ca)?;
            let port=ssh_transport::start(&target, ssh_auth.clone(), &mut session, local)?;
            setup = Some(ssh_transport::SetupGuard::new(&target));
            port
        };
        target.connection_mode = ssh_transport::Mode::Ssh;
        target.ssh_remote_port = Some(port);
        request.server_url = format!("https://127.0.0.1:{port}");
        target.server_url = Some(request.server_url.clone());
    } else {
        if pooled { return Err(OperationsError::bad_request("原 SSH 长连接仍在运行，请先断开后再选择仅 HTTPS 直连重新部署")); }
        target.connection_mode=ssh_transport::Mode::Direct;
        target.ssh_remote_port=None;
    }
    // Resolve transport policy before inserting a pending node. A refused SSH
    // forward must not leave a phantom machine that cannot be deployed again.
    handle.block_on(async {
        let mut tx=state.pool.begin().await.map_err(|_|OperationsError::busy())?;
        if existing.is_some() {
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))").bind(id.to_string()).execute(&mut *tx).await.map_err(|_|OperationsError::busy())?;
            let allowed=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM nodes n WHERE id=$1 AND deleted_at IS NULL AND (NOT monitoring_paused OR EXISTS(SELECT 1 FROM agent_service_controls s WHERE s.node_id=n.id AND s.state='redeploying')))")
                .bind(id).fetch_one(&mut *tx).await.map_err(|_|OperationsError::busy())?;
            if !allowed { return Err(OperationsError::bad_request("原机器已被暂停或软删除；已停止重新部署，未改注册凭据")); }
        } else {
        sqlx::query("INSERT INTO nodes(id,hostname,display_name,os_name,os_version,kernel_version,architecture,cpu_model,cpu_logical_cores,agent_version,last_seen) VALUES($1,$2,$2,'等待 Agent','','',$3,'',0,'等待 Agent',NOW()-INTERVAL '1 day')")
            .bind(id).bind(&request.display_name).bind(fields[2]).execute(&mut *tx).await.map_err(|_|OperationsError::busy())?;
        }
        sqlx::query("INSERT INTO machine_enrollments(node_id,token_hash,expires_at,expected_user) VALUES($1,$2,NOW()+INTERVAL '10 minutes',$3) ON CONFLICT(node_id) DO UPDATE SET token_hash=EXCLUDED.token_hash,expires_at=EXCLUDED.expires_at,expected_user=EXCLUDED.expected_user")
            .bind(id).bind(crate::api::hash_token(&bootstrap)).bind(&request.username).execute(&mut *tx).await.map_err(|_|OperationsError::busy())?;
        sqlx::query("INSERT INTO audit_logs(actor_type,actor_id,action,target_type,target_id,details) VALUES('ssh-user',$1,$3,'node',$2,'{\"result\":\"authorized\"}')")
            .bind(&request.username).bind(id.to_string()).bind(if existing.is_some(){"machine.redeploy"}else{"machine.add"}).execute(&mut *tx).await.map_err(|_|OperationsError::busy())?;
        tx.commit().await.map_err(|_|OperationsError::busy())
    })?;
    crate::read_cache::invalidate();
    save_target(state, &target)?;
    ssh_vault::save_with_token(&state.registry,&target,&ssh_auth,&request.sudo_password,false,browser_token)?;
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
        "/usr/bin/timeout --signal=TERM --kill-after=3s 90s /bin/bash {}{}",
        shell_quote(&format!("{stage}/install-user-agent.sh")), if existing.is_some(){" --repair"}else{""}
    );
    let previous_pid = if existing.is_some() {
        handle.block_on(sqlx::query_scalar::<_,Option<String>>("SELECT metrics->'platform_process'->>'pid' FROM metrics_latest WHERE node_id=$1")
            .bind(id).fetch_optional(&state.pool)).map_err(|_|OperationsError::busy())?.flatten()
    } else { None };
    let installation_started=Utc::now();
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
        let invocation = format!("/usr/bin/timeout --signal=TERM --kill-after=3s 90s /usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin /bin/bash --noprofile --norc -c {} -- {} {} {} {} {} {} {}",
            shell_quote(FULL_INSTALL), shell_quote(&stage), shell_quote(&target.release_sha256.clone().unwrap_or_default()),
            shell_quote(target.ca_sha256.as_deref().unwrap_or_default()), shell_quote(&hex::encode(Sha256::digest(env.as_bytes()))),
            shell_quote(&id.to_string()), shell_quote(&request.server_url),if existing.is_some(){"repair"}else{"new"});
        let result = if let Some(credentials)=administrator.as_ref() {run_privileged_on(admin_session.as_deref().unwrap_or(&session),credentials.clone(),&invocation)}
            else { admin_command(&session, &request, &invocation) };
        request.sudo_password.zeroize();
        result?;
    }
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
            sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM nodes n JOIN metrics_latest m ON m.node_id=n.id JOIN agents a ON a.node_id=n.id WHERE n.id=$1 AND n.deleted_at IS NULL AND n.last_seen>NOW()-INTERVAL '30 seconds' AND m.metrics->'privilege'->>'mode'=$2 AND m.metrics->'privilege'->>'uid'=$3 AND a.enrolled_at>=$4 AND m.updated_at>=$4 AND (m.metrics->>'collected_at')::timestamptz>=$4 AND (NOT $5 OR m.metrics->'platform_process'->>'pid' IS NOT NULL) AND ($6::text IS NULL OR m.metrics->'platform_process'->>'pid' IS DISTINCT FROM $6))")
                .bind(id).bind(mode.name()).bind(if mode == DeploymentMode::Basic { uid.to_string() } else { "0".to_owned() }).bind(installation_started).bind(existing.is_some()).bind(previous_pid.as_deref()).fetch_one(&state.pool).await
        }).unwrap_or(false);
        if confirmed {
            target.agent_sha256 = target.release_sha256.clone();
            save_target(state, &target)?;
            ssh_vault::set_enabled(&state.registry,&target,target.connection_mode==ssh_transport::Mode::Ssh)?;
            if let Some(setup) = setup.as_mut() { setup.commit(); }
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
    // Keep registry mutations exclusive with redeployment, while preserving
    // their existing machine-management rate limit rather than charging twice.
    let _node_lease=acquire_node_lease(&state.limits,id,true,false)?;
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
        ..Default::default()
    };
    let actor = credentials.username.clone();
    validate_credentials(&credentials)?;
    let admin = request.auth_scope == "platform_admin";
    if !matches!(request.auth_scope.as_str(), "target" | "platform_admin") {
        return Err(OperationsError::bad_request("鉴权目标不正确"));
    }
    let mut check = if admin {
        platform_target(&state)?
    } else {
        target.clone().ok_or(OperationsError::bad_request(
            "机器缺少可信目标；请使用平台管理员鉴权",
        ))?
    };
    // Restoring/editing a disconnected SSH node is an explicit credentialed
    // operation, not a background probe. Do not require an already-live tunnel
    // to authenticate the request that will permit reconnecting it.
    if check.connection_mode==ssh_transport::Mode::Ssh && ssh_transport::status(&check)["status"]=="disconnected" {
        check.connection_mode=ssh_transport::Mode::Direct;
    }
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
    crate::agent_auth::invalidate();
    if request.action == "delete" && let Some(target) = target.as_ref() {
        ssh_transport::stop(target)?;
        if ssh_vault::exists(&state.registry,target) { ssh_vault::set_enabled(&state.registry,target,false)?; }
    }
    Ok(Json(
        json!({"message":match request.action.as_str(){"delete"=>"已软删除：历史、身份和远端文件保留，上报凭据停用", "restore"=>"已恢复原机器身份与上报凭据，等待 Agent 心跳", _=>"显示名称和备注已更新；SSH 地址、权限和机器身份未改变"}}),
    ))
}

fn verify_admin(session: &Session, credentials: &Credentials) -> Result<(), OperationsError> {
    if command(session, "/usr/bin/id -u")?.trim() == "0" {
        return Ok(());
    }
    let mut channel = crate::network::ssh_channel(&session)
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

    #[test]
    fn redeployment_preserves_full_privileges_and_disallows_target_overrides() {
        let mut target: Target=serde_json::from_value(json!({"node_id":Uuid::new_v4(),"hostname":"fixture",
            "host":"10.0.0.9","port":22,"default_user":"tester","agent_home":"/home/tester/.easymanage","known_hosts":"/tmp/fixture-known-hosts"})).unwrap();
        assert!(preserved_mode(&target,None)==DeploymentMode::Basic);
        assert!(preserved_mode(&target,Some("full"))==DeploymentMode::Full);
        target.agent_mode=Some("full".into());
        assert!(preserved_mode(&target,Some("basic"))==DeploymentMode::Full);
        target.agent_mode=None;target.default_user="root".into();
        assert!(preserved_mode(&target,None)==DeploymentMode::Full);
        let request=json!({"confirmation":target.node_id,"username":"tester","server_url":"https://127.0.0.1:8443","use_saved":true});
        assert!(serde_json::from_value::<RedeployRequest>(request.clone()).is_ok());
        for field in ["host","port","node_id","deployment_mode","known_hosts","force"] {
            let mut altered=request.clone();altered[field]=json!("cannot-override");
            assert!(serde_json::from_value::<RedeployRequest>(altered).is_err());
        }
    }

    #[test]
    fn redeployment_rotates_finished_job_capability_but_never_replaces_a_running_job() {
        let tasks=Tasks::default();let node=Uuid::new_v4();let host="10.0.0.9".parse().unwrap();
        let first=tasks.begin(node,host).unwrap();
        assert!(tasks.begin(node,host).is_err());
        assert!(tasks.begin(Uuid::new_v4(),host).is_err());
        tasks.event(node,100,"failed","fixed sanitized fixture failure");
        let next=tasks.begin(node,host).unwrap();
        assert_ne!(*first,*next);
        let jobs=tasks.jobs.lock().unwrap();
        assert_eq!(jobs.len(),1);
        assert_eq!(jobs[&node].node_id,node);
        assert_eq!(jobs[&node].key_hash,crate::api::hash_token(&next));
        assert_ne!(jobs[&node].key_hash,*next);
    }
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
