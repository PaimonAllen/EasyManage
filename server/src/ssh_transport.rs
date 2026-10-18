//! Controller-initiated, bounded SSH transports. Credentials at rest use ssh_vault.
//! Two long-lived TCP sessions (control + reverse HTTPS), plus one optional admin
//! identity. Channels are NOT connections. Control operations serialize because
//! libssh2 blocking mode belongs to a session, not to an individual channel.
use super::*;
use base64::Engine as _;
use std::ops::Deref;
use std::sync::OnceLock;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

fn fingerprint(session: &Session) -> Result<String, OperationsError> {
    let (key, _) = session.host_key().ok_or(OperationsError::registry())?;
    Ok(format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(key))
    ))
}

pub(super) const MAX_CONNECTIONS: usize = 3;
pub(super) const RETRY_LIMIT: u32 = 3;
pub(super) const RETRY_INTERVAL: Duration = Duration::from_secs(60);
const MAX_PIPES: usize = 8;
const PROBE_TTL: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Mode {
    Auto,
    #[default]
    Direct,
    Ssh,
}

pub(super) const fn automatic() -> Mode {
    Mode::Auto
}

// Not Debug/Serialize. Zeroized when the worker/last credential owner stops.
#[derive(Clone)]
pub(super) struct Auth {
    pub username: String,
    pub password: Zeroizing<String>,
    pub key: Zeroizing<String>,
    pub passphrase: Zeroizing<String>,
}

impl Auth {
    fn identity(&self) -> String {
        let mut hash = Sha256::new();
        for value in [
            &self.username,
            &*self.password,
            &*self.key,
            &*self.passphrase,
        ] {
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value.as_bytes());
        }
        hex::encode(hash.finalize())
    }

    pub(super) fn authenticate(&self, session: &Session) -> Result<(), OperationsError> {
        let result = if self.key.is_empty() {
            session.userauth_password(&self.username, &self.password)
        } else {
            session.userauth_pubkey_memory(
                &self.username,
                None,
                &self.key,
                if self.passphrase.is_empty() {
                    None
                } else {
                    Some(&self.passphrase)
                },
            )
        };
        result.map_err(|_| {
            OperationsError(
                StatusCode::UNAUTHORIZED,
                "SSH 长连接认证失败，请重新提交有效凭据",
            )
        })?;
        if !session.authenticated() {
            return Err(OperationsError::ssh("SSH 认证未完成"));
        }
        session.set_keepalive(true, 30);
        Ok(())
    }
}

struct Connection {
    session: Session,
    socket: TcpStream,
    _slot: OwnedSemaphorePermit,
}

struct Cached {
    connection: Arc<Connection>,
    serial: Arc<Semaphore>,
    identity: String,
}

impl Cached {
    fn new(connection: Arc<Connection>, identity: String) -> Self {
        Self {
            connection,
            serial: Arc::new(Semaphore::new(1)),
            identity,
        }
    }
    fn lease(&self) -> Result<SessionLease, OperationsError> {
        Ok(SessionLease {
            connection: self.connection.clone(),
            _serial: Some(
                self.serial
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| OperationsError::busy())?,
            ),
        })
    }
}

pub(super) struct SessionLease {
    connection: Arc<Connection>,
    _serial: Option<OwnedSemaphorePermit>,
}
impl Deref for SessionLease {
    type Target = Session;
    fn deref(&self) -> &Session {
        &self.connection.session
    }
}
impl Drop for SessionLease {
    fn drop(&mut self) {
        self.set_blocking(true);
        self.set_timeout(10_000);
    }
}

#[derive(Default)]
struct State {
    probe: Option<(Arc<Connection>, Instant)>,
    control: Option<Cached>,
    admin: Option<Cached>,
    stop: Option<Arc<AtomicBool>>,
    worker_done: Option<Arc<AtomicBool>>,
    tunnel_socket: Option<TcpStream>,
    node_id: Option<Uuid>,
    status: &'static str,
    reconnects: u64,
    retry_attempt: u32,
    last_error: Option<&'static str>,
    port: Option<u16>,
}

struct Host {
    slots: Arc<Semaphore>,
    // Serializes physical connection creation, including reconnection and admin login.
    dial: Mutex<()>,
    state: Mutex<State>,
}
impl Default for Host {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
            dial: Mutex::new(()),
            state: Mutex::new(State::default()),
        }
    }
}

type Hosts = Mutex<HashMap<SocketAddr, Arc<Host>>>;
static HOSTS: OnceLock<Hosts> = OnceLock::new();
fn hosts() -> &'static Hosts {
    HOSTS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn host(address: SocketAddr) -> Result<Arc<Host>, OperationsError> {
    let mut hosts = hosts().lock().map_err(|_| OperationsError::busy())?;
    if !hosts.contains_key(&address) && hosts.len() >= 256 {
        // Reclaim only idle endpoints; never evict a transport with live connections.
        hosts.retain(|_, host| host.slots.available_permits() != MAX_CONNECTIONS);
        if hosts.len() >= 256 {
            return Err(OperationsError::busy());
        }
    }
    Ok(hosts.entry(address).or_default().clone())
}

pub(super) fn maintenance() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(Duration::from_secs(15));
                if let Ok(hosts) = hosts().lock() {
                    for host in hosts.values() {
                        if let Ok(mut state) = host.state.lock()
                            && state
                                .probe
                                .as_ref()
                                .is_some_and(|(_, at)| at.elapsed() > PROBE_TTL)
                        {
                            state.probe = None;
                        }
                    }
                }
            }
        });
    });
}

fn dial(host: &Host, address: SocketAddr) -> Result<Arc<Connection>, OperationsError> {
    let slot = host
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| OperationsError::busy())?;
    let socket = TcpStream::connect_timeout(&address, Duration::from_secs(8))
        .map_err(|_| OperationsError::ssh("SSH 连接失败"))?;
    socket
        .set_nodelay(true)
        .map_err(|_| OperationsError::ssh("SSH socket 初始化失败"))?;
    let mut session = Session::new().map_err(|_| OperationsError::ssh("SSH 初始化失败"))?;
    session.set_tcp_stream(
        socket
            .try_clone()
            .map_err(|_| OperationsError::ssh("SSH socket 初始化失败"))?,
    );
    session.set_timeout(10_000);
    session
        .handshake()
        .map_err(|_| OperationsError::ssh("SSH 握手失败"))?;
    Ok(Arc::new(Connection {
        session,
        socket,
        _slot: slot,
    }))
}

fn verify(connection: &Connection, target: &Target) -> Result<(), OperationsError> {
    check_private_file(&target.known_hosts)?;
    let mut known = connection
        .session
        .known_hosts()
        .map_err(|_| OperationsError::registry())?;
    known
        .read_file(&target.known_hosts, KnownHostFileKind::OpenSSH)
        .map_err(|_| OperationsError::registry())?;
    let (key, _) = connection
        .session
        .host_key()
        .ok_or(OperationsError::registry())?;
    if !matches!(
        known.check_port(&target.host.to_string(), target.port, key),
        CheckResult::Match
    ) {
        return Err(OperationsError(
            StatusCode::CONFLICT,
            "SSH 主机密钥发生变化；长连接已停止，请管理员核验",
        ));
    }
    Ok(())
}

// Reuse the unauthenticated fingerprint-probe handshake for deployment, or an
// existing control connection for another probe. No credentials sent by probe.
pub(super) fn probe(address: SocketAddr) -> Result<String, OperationsError> {
    let host = host(address)?;
    let _dial = host.dial.lock().map_err(|_| OperationsError::busy())?;
    let mut state = host.state.lock().map_err(|_| OperationsError::busy())?;
    if let Some(control) = &state.control {
        return fingerprint(&control.connection.session);
    }
    if state
        .probe
        .as_ref()
        .is_some_and(|(_, at)| at.elapsed() > PROBE_TTL)
    {
        state.probe = None;
    }
    if state.probe.is_none() {
        state.probe = Some((dial(&host, address)?, Instant::now()));
    }
    fingerprint(
        &state
            .probe
            .as_ref()
            .ok_or(OperationsError::busy())?
            .0
            .session,
    )
}

pub(super) fn onboarding(address: SocketAddr) -> Result<SessionLease, OperationsError> {
    let host = host(address)?;
    let _dial = host.dial.lock().map_err(|_| OperationsError::busy())?;
    let mut state = host.state.lock().map_err(|_| OperationsError::busy())?;
    if state.control.is_some() {
        return Err(OperationsError::bad_request(
            "已有 SSH 长连接，请使用原机器的重连入口",
        ));
    }
    let connection = state
        .probe
        .take()
        .filter(|(_, at)| at.elapsed() <= PROBE_TTL)
        .map(|(connection, _)| connection);
    drop(state);
    Ok(SessionLease {
        connection: match connection {
            Some(value) => value,
            None => dial(&host, address)?,
        },
        _serial: None,
    })
}

pub(super) fn operation(
    target: &Target,
    credentials: &Credentials,
    cancellation: &Cancellation,
) -> Result<SessionLease, OperationsError> {
    let auth = Auth {
        username: credentials.username.clone(),
        password: credentials.password.clone(),
        key: Zeroizing::new(String::new()),
        passphrase: Zeroizing::new(String::new()),
    };
    authenticated(target, &auth, cancellation)
}

pub(super) fn authenticated(
    target: &Target,
    auth: &Auth,
    cancellation: &Cancellation,
) -> Result<SessionLease, OperationsError> {
    let host = host(SocketAddr::new(target.host, target.port))?;
    let _dial = host.dial.lock().map_err(|_| OperationsError::busy())?;
    let identity = auth.identity();
    let mut state = host.state.lock().map_err(|_| OperationsError::busy())?;
    if target.connection_mode == Mode::Ssh {
        for cached in [&state.control, &state.admin].into_iter().flatten() {
            if crate::api::constant_time_equal(&cached.identity, &identity) {
                verify(&cached.connection, target)?;
                return cached.lease();
            }
        }
        if state.stop.is_none() {
            return Err(OperationsError::ssh(
                "SSH 长连接未建立；平台重启后需重新提交鉴权",
            ));
        }
        // Never allocate a fourth TCP connection. Replace an unused auxiliary
        // identity only; do not interrupt a terminal or a running admin command.
        if state
            .admin
            .as_ref()
            .is_some_and(|cached| cached.serial.available_permits() == 0)
        {
            return Err(OperationsError::busy());
        }
        state.admin = None;
    }
    drop(state);
    let connection = dial(&host, SocketAddr::new(target.host, target.port))?;
    verify(&connection, target)?;
    auth.authenticate(&connection.session)?;
    if target.connection_mode == Mode::Ssh {
        let cached = Cached::new(connection, identity);
        let lease = cached.lease()?;
        host.state
            .lock()
            .map_err(|_| OperationsError::busy())?
            .admin = Some(cached);
        // Cancellation of one channel must not tear down the shared transport.
        Ok(lease)
    } else {
        *cancellation
            .socket
            .lock()
            .map_err(|_| OperationsError::busy())? = Some(
            connection
                .socket
                .try_clone()
                .map_err(|_| OperationsError::busy())?,
        );
        if cancellation.is_cancelled() {
            cancellation.cancel();
            return Err(OperationsError::ssh("操作已取消"));
        }
        Ok(SessionLease {
            connection,
            _serial: None,
        })
    }
}

pub(super) fn verify_lease(session: &SessionLease, target: &Target) -> Result<(), OperationsError> {
    verify(&session.connection, target)
}

pub(super) fn primary(target: &Target) -> Result<SessionLease, OperationsError> {
    let host = host(SocketAddr::new(target.host, target.port))?;
    let state = host.state.lock().map_err(|_| OperationsError::busy())?;
    let cached = state.control.as_ref().ok_or(OperationsError::ssh(
        "SSH 控制连接尚未恢复；请等待或重新鉴权",
    ))?;
    verify(&cached.connection, target)?;
    cached.lease()
}

pub(super) fn status(target: &Target) -> serde_json::Value {
    let Ok(host) = host(SocketAddr::new(target.host, target.port)) else {
        return json!({"status":"unavailable"});
    };
    let Ok(state) = host.state.lock() else {
        return json!({"status":"busy"});
    };
    json!({"status":if state.status.is_empty() {"disconnected"} else {state.status},
        "connections":MAX_CONNECTIONS - host.slots.available_permits(), "limit":MAX_CONNECTIONS,
        "remote_port":state.port.or(target.ssh_remote_port), "reconnects":state.reconnects,
        "retry_attempt":state.retry_attempt,"retry_limit":RETRY_LIMIT,"retry_interval_seconds":RETRY_INTERVAL.as_secs(),
        "last_error":state.last_error})
}

pub(super) fn restore_status(target: &Target, status: &'static str, attempt: u32, error: &'static str) {
    if let Ok(host) = host(SocketAddr::new(target.host,target.port))
        && let Ok(mut state) = host.state.lock() {
        state.status = status;
        state.retry_attempt = attempt;
        state.last_error = Some(error);
    }
}

pub(super) fn stop(target: &Target) -> Result<(), OperationsError> {
    let host = host(SocketAddr::new(target.host, target.port))?;
    let mut state = host.state.lock().map_err(|_| OperationsError::busy())?;
    if let Some(stop) = state.stop.take() {
        stop.store(true, Ordering::Relaxed);
    }
    if let Some(socket) = state.tunnel_socket.take() {
        let _ = socket.shutdown(Shutdown::Both);
    }
    for cached in [&state.control, &state.admin].into_iter().flatten() {
        let _ = cached.connection.socket.shutdown(Shutdown::Both);
    }
    state.control = None;
    state.admin = None;
    state.status = "disconnected";
    state.last_error = None;
    state.retry_attempt = 0;
    let done = state.worker_done.take();
    drop(state);
    // The libssh2 listener owns the remote port until its worker drops it.
    // Wait for acknowledgement before a manual reconnect can dial/rebind.
    if let Some(done) = done {
        let deadline = Instant::now()+Duration::from_secs(3);
        while !done.load(Ordering::Acquire) && Instant::now()<deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if !done.load(Ordering::Acquire) {
            host.state.lock().map_err(|_|OperationsError::busy())?.worker_done = Some(done);
            return Err(OperationsError::ssh("旧 SSH 会话尚未完全退出；未新建连接，请稍后重试"));
        }
    }
    Ok(())
}

pub(super) struct SetupGuard(Option<Target>);
impl SetupGuard {
    pub(super) fn new(target: &Target) -> Self {
        Self(Some(target.clone()))
    }
    pub(super) fn commit(&mut self) {
        self.0 = None;
    }
}
impl Drop for SetupGuard {
    fn drop(&mut self) {
        if let Some(target) = self.0.as_ref() {
            let _ = stop(target);
        }
    }
}

fn verify_listener(session: &Session, port: u16) -> Result<(), OperationsError> {
    // GatewayPorts=yes can override the client's requested bind address. Verify
    // the actual Linux listener before installing any loopback Agent URL.
    let script = "import sys\np=int(sys.argv[1]);found=[]\nfor f in ('/proc/net/tcp','/proc/net/tcp6'):\n for line in open(f).read().splitlines()[1:]:\n  row=line.split();a,port=row[1].split(':')\n  if int(port,16)==p and row[3]=='0A': found.append(a)\nassert found and all(a=='0100007F' for a in found)\nprint('loopback')";
    let mut check = session
        .channel_session()
        .map_err(|_| OperationsError::ssh("SSH 回环监听核验失败"))?;
    check
        .handle_extended_data(ExtendedData::Ignore)
        .map_err(|_| OperationsError::busy())?;
    check
        .exec(&format!("python3 -c {} {port}", shell_quote(script)))
        .map_err(|_| OperationsError::ssh("SSH 回环监听核验失败"))?;
    let mut output = String::new();
    (&mut check)
        .take(1024)
        .read_to_string(&mut output)
        .map_err(|_| OperationsError::ssh("SSH 回环监听核验失败"))?;
    check
        .wait_close()
        .map_err(|_| OperationsError::ssh("SSH 回环监听核验失败"))?;
    if check.exit_status().unwrap_or(-1) != 0 || output.trim() != "loopback" {
        return Err(OperationsError::ssh(
            "SSH 实际监听不是仅回环地址或无法核验；已拒绝隧道，请管理员核对 GatewayPorts",
        ));
    }
    Ok(())
}

pub(super) fn start(
    target: &Target,
    auth: Auth,
    session: &mut SessionLease,
    local: SocketAddr,
) -> Result<u16, OperationsError> {
    let host = host(SocketAddr::new(target.host, target.port))?;
    let _dial = host.dial.lock().map_err(|_| OperationsError::busy())?;
    {
        let state = host.state.lock().map_err(|_| OperationsError::busy())?;
        if state.stop.is_some() {
            return Err(OperationsError::bad_request(
                "SSH 长连接已存在；请先断开再重新连接",
            ));
        }
    }
    verify(&session.connection, target)?;
    // Own a second authenticated SSH transport; keep control blocking I/O away
    // from reverse-forward traffic. This is the second, not the third, session.
    let tunnel = dial(&host, SocketAddr::new(target.host, target.port))?;
    verify(&tunnel, target)?;
    auth.authenticate(&tunnel.session)?;
    let (listener, port) = tunnel.session.channel_forward_listen(target.ssh_remote_port.unwrap_or(0), Some("127.0.0.1"), Some(MAX_PIPES as u32))
        .map_err(|_| OperationsError::ssh("SSH 拒绝回环端口转发或端口被占用；请管理员核对 AllowTcpForwarding/PermitListen，不会修改 sshd"))?;
    if port == 0 {
        return Err(OperationsError::ssh("SSH 未返回有效的回环监听端口"));
    }
    verify_listener(session, port)?;
    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    {
        let mut state = host.state.lock().map_err(|_| OperationsError::busy())?;
        let cached = Cached::new(session.connection.clone(), auth.identity());
        session._serial = Some(
            cached
                .serial
                .clone()
                .try_acquire_owned()
                .map_err(|_| OperationsError::busy())?,
        );
        state.control = Some(cached);
        state.stop = Some(stop.clone());
        state.worker_done = Some(done.clone());
        state.tunnel_socket = Some(
            tunnel
                .socket
                .try_clone()
                .map_err(|_| OperationsError::busy())?,
        );
        state.node_id = Some(target.node_id);
        state.status = "connected";
        state.retry_attempt = 0;
        state.last_error = None;
        state.port = Some(port);
    }
    let mut target = target.clone();
    target.ssh_remote_port = Some(port);
    let host_clone = host.clone();
    std::thread::spawn(move || {
        struct Done(Arc<AtomicBool>);
        impl Drop for Done { fn drop(&mut self) { self.0.store(true,Ordering::Release); } }
        let _done = Done(done);
        maintain(host_clone, target, auth, stop, local, tunnel, listener);
    });
    Ok(port)
}

fn retry_delay(attempt: u32) -> Option<Duration> {
    (attempt < RETRY_LIMIT).then_some(RETRY_INTERVAL)
}

fn finish(host: &Host, stop: &Arc<AtomicBool>, status: &'static str) {
    if let Ok(mut state) = host.state.lock()
        && state.stop.as_ref().is_some_and(|current| Arc::ptr_eq(current,stop)) {
        for cached in [&state.control,&state.admin].into_iter().flatten() {
            let _ = cached.connection.socket.shutdown(Shutdown::Both);
        }
        state.control = None;
        state.admin = None;
        state.tunnel_socket = None;
        state.stop = None;
        state.status = status;
    }
}

fn maintain(
    host: Arc<Host>,
    target: Target,
    auth: Auth,
    stop: Arc<AtomicBool>,
    local: SocketAddr,
    mut tunnel: Arc<Connection>,
    mut listener: ssh2::Listener,
) {
    loop {
        let mut attempt = 0;
        let result = pump(&host, &tunnel.session, &mut listener, &stop, local);
        // Drop listener and connection before waiting/dialing. No overlapping
        // replacement tunnel can exceed the per-endpoint semaphore budget.
        drop(listener);
        drop(tunnel);
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if let Ok(mut state) = host.state.lock() {
            state.tunnel_socket = None;
            state.retry_attempt = 0;
            state.status = if result.is_err() {
                "reconnecting"
            } else {
                "disconnected"
            };
        }
        loop {
            let Some(delay) = retry_delay(attempt) else {
                finish(&host, &stop, "retry_exhausted");
                return;
            };
            let until = Instant::now() + delay;
            while Instant::now() < until && !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
            }
            if stop.load(Ordering::Relaxed) {
                return;
            }
            attempt += 1;
            if let Ok(mut state) = host.state.lock() { state.retry_attempt = attempt; }
            let result = (|| {
                let _dial = host.dial.lock().map_err(|_| OperationsError::busy())?;
                if stop.load(Ordering::Relaxed) {
                    return Err(OperationsError::busy());
                }
                let new = dial(&host, SocketAddr::new(target.host, target.port))?;
                verify(&new, &target)?;
                auth.authenticate(&new.session)?;
                let (listener, port) = new
                    .session
                    .channel_forward_listen(
                        target.ssh_remote_port.unwrap_or(0),
                        Some("127.0.0.1"),
                        Some(MAX_PIPES as u32),
                    )
                    .map_err(|_| OperationsError::ssh("SSH 回环转发重建失败"))?;
                if Some(port) != target.ssh_remote_port {
                    return Err(OperationsError::ssh("SSH 重连监听端口改变，未继续转发"));
                }
                let needs_control = host
                    .state
                    .lock()
                    .map_err(|_| OperationsError::busy())?
                    .control
                    .is_none();
                let mut control = if needs_control {
                    let connection = dial(&host, SocketAddr::new(target.host, target.port))?;
                    verify(&connection, &target)?;
                    auth.authenticate(&connection.session)?;
                    Some(Cached::new(connection, auth.identity()))
                } else {
                    None
                };
                let control_lease = if let Some(control) = control.as_ref() {
                    control.lease()?
                } else {
                    host.state
                        .lock()
                        .map_err(|_| OperationsError::busy())?
                        .control
                        .as_ref()
                        .ok_or(OperationsError::busy())?
                        .lease()?
                };
                if let Err(error) = verify_listener(&control_lease, port) {
                    // An outage may kill BOTH sessions. The forward can recover
                    // before the old control socket notices EOF; do not retain
                    // that dead cached control forever across retries.
                    drop(control_lease);
                    if control.is_some() { return Err(error); }
                    host.state.lock().map_err(|_| OperationsError::busy())?.control = None;
                    // Rebuild the stale control within THIS retry, not after
                    // another minute. Release its lease/cache before dialing.
                    let connection = dial(&host,SocketAddr::new(target.host,target.port))?;
                    verify(&connection,&target)?;
                    auth.authenticate(&connection.session)?;
                    let replacement = Cached::new(connection,auth.identity());
                    let check = replacement.lease()?;
                    verify_listener(&check,port)?;
                    drop(check);
                    control = Some(replacement);
                } else { drop(control_lease); }
                let mut state = host.state.lock().map_err(|_| OperationsError::busy())?;
                if stop.load(Ordering::Relaxed) {
                    return Err(OperationsError::busy());
                }
                state.tunnel_socket = Some(
                    new.socket
                        .try_clone()
                        .map_err(|_| OperationsError::busy())?,
                );
                if let Some(control) = control {
                    state.control = Some(control);
                }
                state.reconnects += 1;
                state.status = "connected";
                state.last_error = None;
                Ok((new, listener))
            })();
            match result {
                Ok((new, new_listener)) => {
                    tunnel = new;
                    listener = new_listener;
                    break;
                }
                Err(error)
                    if error.0 == StatusCode::UNAUTHORIZED || error.0 == StatusCode::CONFLICT =>
                {
                    if let Ok(mut state) = host.state.lock() { state.last_error = Some(error.1); }
                    finish(&host, &stop, "authentication_required");
                    // No tight retries against bad credentials or changed host keys.
                    return;
                }
                Err(error) => {
                    if let Ok(mut state) = host.state.lock() { state.last_error = Some(error.1); }
                }
            }
        }
    }
}

struct Pipe {
    channel: ssh2::Channel,
    socket: TcpStream,
    to_local: Vec<u8>,
    local_offset: usize,
    to_remote: Vec<u8>,
    remote_offset: usize,
    remote_eof: bool,
    local_eof: bool,
    eof_sent: bool,
    last_io: Instant,
}

fn would_block(error: &ssh2::Error) -> bool {
    error.code() == ssh2::ErrorCode::Session(-37)
}
fn pump(
    host: &Host,
    session: &Session,
    listener: &mut ssh2::Listener,
    stop: &AtomicBool,
    local: SocketAddr,
) -> Result<(), ()> {
    session.set_blocking(false);
    let mut pipes: Vec<Pipe> = Vec::new();
    let mut keepalive = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if keepalive.elapsed() >= Duration::from_secs(15) {
            if let Err(error) = session.keepalive_send()
                && !would_block(&error)
            {
                return Err(());
            }
            // Serialize keepalive with control commands/terminal. No new login.
            let (admin, control) = {
                let state = host.state.lock().map_err(|_| ())?;
                (
                    state.admin.as_ref().and_then(|cached| cached.lease().ok()),
                    state
                        .control
                        .as_ref()
                        .and_then(|cached| cached.lease().ok()),
                )
            };
            // Never hold the registry/state mutex during socket I/O: monitoring
            // and status endpoints must stay responsive during a network outage.
            if let Some(lease) = admin
                && lease.keepalive_send().is_err()
            {
                let mut state = host.state.lock().map_err(|_| ())?;
                if state
                    .admin
                    .as_ref()
                    .is_some_and(|cached| Arc::ptr_eq(&cached.connection, &lease.connection))
                {
                    state.admin = None;
                }
            }
            if let Some(lease) = control
                && lease.keepalive_send().is_err()
            {
                let mut state = host.state.lock().map_err(|_| ())?;
                if state
                    .control
                    .as_ref()
                    .is_some_and(|cached| Arc::ptr_eq(&cached.connection, &lease.connection))
                {
                    state.control = None;
                }
                return Err(());
            }
            keepalive = Instant::now();
        }
        if pipes.len() < MAX_PIPES {
            match listener.accept() {
                Ok(channel) => {
                    // Only the controller's fixed local HTTPS listener is reachable;
                    // remote channel metadata cannot select another destination.
                    if let Ok(socket) = TcpStream::connect_timeout(&local, Duration::from_secs(1)) {
                        socket.set_nonblocking(true).map_err(|_| ())?;
                        let _ = socket.set_nodelay(true);
                        pipes.push(Pipe {
                            channel,
                            socket,
                            to_local: Vec::new(),
                            local_offset: 0,
                            to_remote: Vec::new(),
                            remote_offset: 0,
                            remote_eof: false,
                            local_eof: false,
                            eof_sent: false,
                            last_io: Instant::now(),
                        });
                    }
                }
                Err(error) if would_block(&error) => {}
                Err(_) => return Err(()),
            }
        }
        pipes.retain_mut(|pipe| pipe.tick().unwrap_or(false));
        let active = pipes.iter().any(|pipe| {
            pipe.last_io.elapsed() < Duration::from_millis(200)
                || !pipe.to_local.is_empty()
                || !pipe.to_remote.is_empty()
        });
        std::thread::sleep(Duration::from_millis(if active { 10 } else { 100 }));
    }
    Ok(())
}

impl Pipe {
    fn tick(&mut self) -> std::io::Result<bool> {
        if self.last_io.elapsed() > Duration::from_secs(60) {
            return Ok(false);
        }
        let mut buffer = [0_u8; 16 * 1024];
        if self.to_local.is_empty() && !self.remote_eof {
            match self.channel.read(&mut buffer) {
                Ok(0) => self.remote_eof = self.channel.eof(),
                Ok(size) => {
                    self.to_local.extend_from_slice(&buffer[..size]);
                    self.last_io = Instant::now();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
        }
        if !self.to_local.is_empty() {
            match self.socket.write(&self.to_local[self.local_offset..]) {
                Ok(0) => return Ok(false),
                Ok(size) => {
                    self.local_offset += size;
                    self.last_io = Instant::now();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
            if self.local_offset == self.to_local.len() {
                self.to_local.clear();
                self.local_offset = 0;
            }
        }
        if self.remote_eof && self.to_local.is_empty() {
            let _ = self.socket.shutdown(Shutdown::Write);
        }
        if self.to_remote.is_empty() && !self.local_eof {
            match self.socket.read(&mut buffer) {
                Ok(0) => self.local_eof = true,
                Ok(size) => {
                    self.to_remote.extend_from_slice(&buffer[..size]);
                    self.last_io = Instant::now();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
        }
        if !self.to_remote.is_empty() {
            // Do not alter this buffer after EAGAIN. libssh2 requires retries
            // with the same buffer/length until the pending write completes.
            match self.channel.write(&self.to_remote[self.remote_offset..]) {
                Ok(0) => return Ok(false),
                Ok(size) => {
                    self.remote_offset += size;
                    self.last_io = Instant::now();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
            if self.remote_offset == self.to_remote.len() {
                self.to_remote.clear();
                self.remote_offset = 0;
            }
        }
        if self.local_eof && self.to_remote.is_empty() && !self.eof_sent {
            match self.channel.send_eof() {
                Ok(()) => self.eof_sent = true,
                Err(error) if would_block(&error) => {}
                Err(_) => return Ok(false),
            }
        }
        Ok(!(self.remote_eof
            && self.local_eof
            && self.to_local.is_empty()
            && self.to_remote.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn budget_includes_inflight_handshakes_and_releases_on_drop() {
        let host = Host::default();
        let slots: Vec<_> = (0..3)
            .map(|_| host.slots.clone().try_acquire_owned().unwrap())
            .collect();
        assert!(host.slots.clone().try_acquire_owned().is_err());
        drop(slots);
        assert_eq!(host.slots.available_permits(), MAX_CONNECTIONS);
    }
    #[test]
    fn reconnects_wait_one_minute_and_stop_after_three_attempts() {
        for attempt in 0..3 { assert_eq!(retry_delay(attempt),Some(Duration::from_secs(60))); }
        for attempt in 3..100 { assert_eq!(retry_delay(attempt),None); }
    }
    #[test]
    fn exhausted_worker_cleanup_never_stops_a_newer_transport() {
        let host = Host::default();
        let old = Arc::new(AtomicBool::new(false));
        let current = Arc::new(AtomicBool::new(false));
        host.state.lock().unwrap().stop = Some(current.clone());
        finish(&host,&old,"retry_exhausted");
        assert!(host.state.lock().unwrap().stop.is_some());
        finish(&host,&current,"retry_exhausted");
        let state = host.state.lock().unwrap();
        assert!(state.stop.is_none());
        assert_eq!(state.status,"retry_exhausted");
        assert_eq!(host.slots.available_permits(),3);
    }
    #[test]
    fn legacy_targets_default_to_direct_but_onboarding_defaults_to_auto() {
        assert!(serde_json::from_str::<Mode>("\"bad\"").is_err());
        assert!(Mode::default() == Mode::Direct && automatic() == Mode::Auto);
    }
}
