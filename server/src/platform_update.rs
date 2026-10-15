//! Platform-only, administrator-authorized installation of a locally published candidate.
use super::*;

const INSTALLER: &str = include_str!("../../deploy/linux/operations/platform-release.py");
#[derive(Clone)]
pub(super) struct Cache {
    status: Arc<tokio::sync::Mutex<Option<(Instant, serde_json::Value)>>>,
    running_sha256: Option<String>,
}

impl Default for Cache {
    fn default() -> Self {
        // Hash the loaded executable once, not a potentially replaced on-disk pathname.
        let running_sha256 = (|| -> std::io::Result<String> {
            let mut executable = std::fs::File::open("/proc/self/exe")?;
            let mut hash = Sha256::new();
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let read = executable.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hash.update(&buffer[..read]);
            }
            Ok(hex::encode(hash.finalize()))
        })()
        .ok();
        Self {
            status: Arc::default(),
            running_sha256,
        }
    }
}

fn check_running(result: &mut serde_json::Value, running: Option<&str>) {
    result["running_sha256"] = json!(running);
    if running.is_none() {
        result["update_available"] = json!(false);
        result["message"] = json!("无法确认当前平台二进制摘要，更新暂不可用");
    } else if result["candidate_server_sha256"]
        .as_str()
        .is_some_and(|candidate| Some(candidate) != running)
    {
        result["update_available"] = json!(true);
        result["message"] = json!("有平台更新可安装");
    }
}
const SCHEDULER: &str = r#"
set -Eeuo pipefail
(( EUID == 0 )) || exit 64
owner=$1
code=$2
sha=$3
url=$4
[[ "$owner" =~ ^[a-zA-Z_][a-zA-Z0-9_.\$-]{0,63}$ && "$owner" != root ]] || exit 64
uid=$(/usr/bin/id -u "$owner")
(( uid >= 1000 )) || exit 64
env_args=(/usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin HOME="/home/$owner"
    XDG_RUNTIME_DIR="/run/user/$uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$uid/bus")
user_ctl() { /usr/sbin/runuser -u "$owner" -- "${env_args[@]}" /usr/bin/systemctl --user "$@"; }
[[ "$(user_ctl show easymanage-server.service -p LoadState --value)" == loaded ]] || exit 3
expected="/home/$owner/.local/share/easymanage/bin/easymanage-server"
[[ "$(user_ctl show easymanage-server.service -p ExecStart --value)" == *"path=$expected ;"* ]] || exit 3
user_ctl is-active --quiet easymanage-server.service
/usr/sbin/runuser -u "$owner" -- "${env_args[@]}" /usr/bin/python3 -c "$code" check --sha256 "$sha"
# One fixed transient system unit serializes schedules even across HTTP requests/restarts.
# It runs as the owner, outside the platform cgroup. No Agent unit is touched.
/usr/bin/systemd-run --quiet --collect --unit="easymanage-platform-update-$uid" --on-active=3s \
    --timer-property=AccuracySec=1s --property=TimeoutStartSec=240s \
    /usr/sbin/runuser -u "$owner" -- "${env_args[@]}" /usr/bin/python3 -c "$code" install --sha256 "$sha" --url "$url"
/usr/sbin/runuser -u "$owner" -- "${env_args[@]}" /usr/bin/python3 -c "$code" scheduled --sha256 "$sha"
printf 'Platform update scheduled.\n'
"#;

#[derive(Deserialize)]
struct UpdatePlatformRequest {
    #[serde(flatten)]
    credentials: Credentials,
    candidate_sha256: String,
}

pub(super) fn router() -> Router<OperationsState> {
    Router::new().route("/api/v1/platform/update", get(status).post(update))
}

fn configured(state: &OperationsState) -> bool {
    platform_target(state).is_ok_and(|target| {
        target.agent_home.join("web") == state.config.web_root
            && target.agent_home.join("config/server.env")
                == std::env::var_os("EASYMANAGE_ENV_FILE")
                    .map(PathBuf::from)
                    .unwrap_or_default()
    })
}

fn read_status() -> Result<serde_json::Value, OperationsError> {
    let output = Command::new("/usr/bin/timeout")
        .args(["8s", "/usr/bin/python3", "-c", INSTALLER, "status"])
        .output()
        .map_err(|_| OperationsError::registry())?;
    if !output.status.success() || output.stdout.len() > 8192 {
        return Err(OperationsError::registry());
    }
    serde_json::from_slice(&output.stdout).map_err(|_| OperationsError::registry())
}

async fn status(State(state): State<OperationsState>) -> Json<serde_json::Value> {
    let supported = configured(&state);
    // Coalesce public polling; one bounded Python check per platform per 15 seconds.
    let mut cache = state.platform_update_cache.status.lock().await;
    let mut result = if let Some((at, value)) = cache.as_ref()
        && at.elapsed() < Duration::from_secs(15)
    {
        value.clone()
    } else {
        let value = tokio::task::spawn_blocking(read_status)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_else(|| {
                json!({"update_available":false,
                "message":"无法检查平台候选，请确认 Python 3 和发布目录"})
            });
        *cache = Some((Instant::now(), value.clone()));
        value
    };
    check_running(
        &mut result,
        state.platform_update_cache.running_sha256.as_deref(),
    );
    result["supported"] = json!(supported);
    if !supported {
        result["update_available"] = json!(false);
        result["message"] = json!("仅支持已配置本机可信 SSH 目标的标准 Linux 普通用户平台部署");
    }
    if matches!(
        result["last_result"]["status"].as_str(),
        Some("scheduled" | "installing")
    ) {
        result["update_available"] = json!(false);
        result["message"] = json!("平台更新进行中，请等待更新结果");
    }
    Json(result)
}

async fn update(
    State(state): State<OperationsState>,
    uri: Uri,
    headers: HeaderMap,
    Json(request): Json<UpdatePlatformRequest>,
) -> Result<Json<serde_json::Value>, OperationsError> {
    validate_origin(&headers, &uri)?;
    validate_credentials(&request.credentials)?;
    if !configured(&state) || !valid_digest(&request.candidate_sha256) {
        return Err(OperationsError::bad_request(
            "平台部署配置不支持更新，或候选摘要不合法",
        ));
    }
    let target = platform_target(&state)?;
    let (_, lease) = target_and_lease(&state, target.node_id, true)?;
    let mut current = tokio::task::spawn_blocking(read_status)
        .await
        .map_err(|_| OperationsError::busy())??;
    check_running(
        &mut current,
        state.platform_update_cache.running_sha256.as_deref(),
    );
    if matches!(
        current["last_result"]["status"].as_str(),
        Some("scheduled" | "installing")
    ) {
        return Err(OperationsError::bad_request(
            "平台更新已在进行中，请先核对结果，勿重复提交",
        ));
    }
    if current["update_available"] != true
        || current["candidate_sha256"] != request.candidate_sha256
    {
        return Err(OperationsError::bad_request(
            "候选已改变或没有更新，请重新刷新并确认",
        ));
    }
    let actor = request.credentials.username.clone();
    let sha = request.candidate_sha256;
    let bind = state.config.bind_address;
    let ip = if bind.ip().is_unspecified() {
        if bind.is_ipv6() {
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        } else {
            IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        }
    } else {
        bind.ip()
    };
    let url = format!("https://{}", SocketAddr::new(ip, bind.port()));
    tracing::warn!(%actor, %sha, "platform update requested; administrator authentication required");
    let cancellation = Arc::new(Cancellation::default());
    let _guard = CancelOnDrop(cancellation.clone());
    let worker = tokio::task::spawn_blocking(move || {
        let _lease = lease;
        let invocation = format!(
            "/usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin /usr/bin/timeout --kill-after=2s 45s /bin/bash --noprofile --norc -c {} -- {} {} {} {}",
            shell_quote(SCHEDULER),
            shell_quote(&target.default_user),
            shell_quote(INSTALLER),
            shell_quote(&sha),
            shell_quote(&url)
        );
        run_privileged_command(&target, request.credentials, &cancellation, &invocation)
    });
    let result = tokio::time::timeout(Duration::from_secs(65), worker)
        .await
        .map_err(|_| OperationsError::ssh("平台更新提交超时，请核对更新状态，勿重复提交"))?
        .map_err(|_| OperationsError::ssh("平台更新提交失败"))?;
    tracing::warn!(%actor, success=result.is_ok(), "platform update schedule result");
    result?;
    // A same-candidate retry must not see a cached success from an earlier install.
    *state.platform_update_cache.status.lock().await = None;
    Ok(Json(
        json!({"status":"pending", "message":"平台更新已安排；连接将短暂中断。稍后检查更新结果并刷新页面，Agent 权限与数据不变"}),
    ))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    #[test]
    fn copied_but_not_restarted_platform_still_needs_update() {
        let mut status =
            serde_json::json!({"update_available":false,"candidate_server_sha256":"new"});
        super::check_running(&mut status, Some("old"));
        assert_eq!(status["update_available"], true);
        super::check_running(&mut status, None);
        assert_eq!(status["update_available"], false);
    }
    #[test]
    fn platform_digest_is_not_a_command_or_path() {
        assert!(super::valid_digest(&"a".repeat(64)));
        for value in ["", "$(reboot)", "../candidate", "ffffffff"] {
            assert!(!super::valid_digest(value));
        }
    }
}
