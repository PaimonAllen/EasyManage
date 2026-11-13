//! Two-step, browser-bound approval for sensitive metadata scans only.
use super::*;
const REVIEW_TTL: Duration = Duration::from_secs(300);
pub(super) struct Review {
    binding: String,
    issued: Instant,
}

pub(super) fn sensitive(path: &str, scope: &str, windows: bool) -> bool {
    if scope == "all_local" {
        return true;
    }
    let normalized = path.replace('\\', "/");
    let parts: Vec<_> = if windows {
        normalized
            .get(3..)
            .unwrap_or("")
            .split('/')
            .filter(|p| !p.is_empty())
            .collect()
    } else {
        path.split('/').filter(|p| !p.is_empty()).collect()
    };
    if parts.len() <= 1 {
        return true;
    }
    if windows {
        matches!(
            parts[0].to_ascii_lowercase().as_str(),
            "windows"
                | "program files"
                | "program files (x86)"
                | "programdata"
                | "system volume information"
                | "recovery"
                | "$recycle.bin"
        )
    } else {
        matches!(
            parts[0],
            "root"
                | "etc"
                | "boot"
                | "usr"
                | "var"
                | "bin"
                | "sbin"
                | "lib"
                | "lib32"
                | "lib64"
                | "libx32"
                | "proc"
                | "sys"
                | "dev"
                | "run"
                | "opt"
                | "srv"
        )
    }
}

pub(super) fn normalize(target: &Target, request: &mut Request) -> Result<(), OperationsError> {
    if target.platform.is_windows() {
        request.path = request.path.replace('\\', "/");
    }
    validate(request)?;
    if target.platform.is_windows() {
        if request.scope != "all_local" && !valid_windows_path(&request.path) {
            return Err(OperationsError::bad_request(
                "Windows 扫描需要本地磁盘绝对路径",
            ));
        }
    } else {
        if !request.path.starts_with('/') {
            return Err(OperationsError::bad_request(
                "Linux 扫描需要 POSIX 绝对路径",
            ));
        }
        request.path = format!(
            "/{}",
            request
                .path
                .split('/')
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("/")
        );
    }
    validate(request)
}
fn scanner_hash(target: &Target) -> String {
    hex::encode(Sha256::digest(if target.platform.is_windows() {
        include_bytes!("../../../deploy/windows/scan-disk-space.ps1").as_slice()
    } else {
        SCRIPT.as_bytes()
    }))
}
fn binding(target: &Target, request: &Request, browser: &str) -> Result<String, OperationsError> {
    // Pin the exact trusted target, host keys, fixed scanner and typed arguments.
    check_private_file(&target.known_hosts)?;
    let keys = std::fs::read(&target.known_hosts).map_err(|_| OperationsError::registry())?;
    let data = json!({"target":target,"keys":hex::encode(Sha256::digest(keys)),"scanner":scanner_hash(target),
        "browser":crate::api::hash_token(browser),"path":request.path,"scope":request.scope,"mode":request.mode,
        "storage":request.storage,"seconds":request.seconds,"max_entries":request.max_entries,"viewer":request.viewer});
    Ok(hex::encode(Sha256::digest(data.to_string().as_bytes())))
}
fn browser_token(
    state: &OperationsState,
    target: &Target,
    headers: &HeaderMap,
) -> Result<Zeroizing<String>, OperationsError> {
    ssh_vault::admin_token(&state.registry, target, headers).map_err(|_| {
        OperationsError(
            StatusCode::UNAUTHORIZED,
            "敏感范围需要当前浏览器验证本机管理员身份",
        )
    })
}
fn rejected() -> OperationsError {
    OperationsError(
        StatusCode::CONFLICT,
        "敏感范围需要管理员验证、通过安全检查并再次确认；确认仅有效 5 分钟且只能使用一次",
    )
}
impl Scans {
    fn consume_review(&self, id: Uuid, expected: &str) -> Result<(), OperationsError> {
        let mut reviews = self.reviews.lock().map_err(|_| OperationsError::busy())?;
        reviews.retain(|_, r| r.issued.elapsed() < REVIEW_TTL);
        let valid = reviews
            .get(&id)
            .is_some_and(|r| crate::api::constant_time_equal(&r.binding, expected));
        if !valid {
            return Err(rejected());
        }
        reviews.remove(&id);
        Ok(())
    }
}
pub(super) fn authorize(
    state: &OperationsState,
    target: &Target,
    headers: &HeaderMap,
    request: &mut Request,
) -> Result<(), OperationsError> {
    if !sensitive(&request.path, &request.scope, target.platform.is_windows())
        && request.review_token.is_none()
        && !request.confirmed
    {
        return Ok(());
    }
    let browser = browser_token(state, target, headers)?;
    if !request.confirmed {
        return Err(rejected());
    }
    let token = request.review_token.ok_or_else(rejected)?;
    state
        .disk_space
        .consume_review(token, &binding(target, request, &browser)?)?;
    request.sensitive_authorized = true;
    Ok(())
}
pub(super) async fn review(
    State(state): State<OperationsState>,
    Path(node): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
    Json(mut request): Json<Request>,
) -> Result<Response, OperationsError> {
    validate_origin(&headers, &uri)?;
    let target = target(&state, node)?;
    normalize(&target, &mut request)?;
    let browser = browser_token(&state, &target, &headers)?;
    let full = reported_agent_mode(&state.pool, node).await?;
    if full {
        request.mode = "full".into();
    }
    if request.mode == "full" && !full {
        return Err(OperationsError::bad_request(
            "root 扫描需要当前在线 Full Agent",
        ));
    }
    let enabled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes WHERE id=$1 AND deleted_at IS NULL AND NOT monitoring_paused)")
        .bind(node).fetch_one(&state.pool).await.map_err(|_|db_error())?;
    if !enabled {
        return Err(OperationsError::bad_request("该机器已暂停监控或已删除"));
    }
    // Construct only our embedded, fixed invocation; no caller command exists.
    if target.platform.is_windows() {
        windows_invocation(&target.pwsh_path, false)?;
    } else {
        let _fixed = invocation(&request, &target.default_user);
    }
    let bound = binding(&target, &request, &browser)?;
    audit(
        &state.pool,
        node,
        &target.default_user,
        "disk-space.security-review",
        "passed",
    )
    .await?;
    let id = Uuid::new_v4();
    {
        let mut reviews = state
            .disk_space
            .reviews
            .lock()
            .map_err(|_| OperationsError::busy())?;
        reviews.retain(|_, r| r.issued.elapsed() < REVIEW_TTL);
        if reviews.len() >= 128 {
            return Err(OperationsError::busy());
        }
        reviews.insert(
            id,
            Review {
                binding: bound,
                issued: Instant::now(),
            },
        );
    }
    Ok(response(
        json!({"review_token":id,"expires_at":Utc::now()+chrono::Duration::minutes(5),"path":request.path,"scope":request.scope,
      "mode":request.mode,"storage":request.storage,"script_sha256":scanner_hash(&target),"read_only":true,
      "checks":["当前浏览器已获得本机管理员授权","目标来自可信机器清单，并绑定 SSH 主机密钥","使用平台内置只读扫描器，未接受自定义命令","绝对路径、扫描范围、时间与条目参数校验通过","路径仅作为转义后的数据传入，不拼接为可执行指令","确认绑定本机、当前浏览器及全部扫描参数，5 分钟内单次有效"],
      "runtime_checks":["实际执行身份校验","逐级拒绝符号链接或重解析点","本地文件系统和挂载标识复核","仅打开目录并读取元数据，不读取文件内容，不修改源文件"]}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roots_and_system_trees_are_sensitive_but_user_data_stays_public() {
        for path in [
            "/",
            "//",
            "/home",
            "/data",
            "/tmp",
            "/etc/a",
            "/root/x",
            "/var/lib/postgresql",
            "/usr/local",
        ] {
            assert!(sensitive(path, "directory", false), "{path}");
        }
        for path in [
            "/home/zbh",
            "/home/zbh/bigdata",
            "/data/data_hdd_3",
            "/tmp/fixture",
            "/etc-backup/data",
        ] {
            assert!(!sensitive(path, "directory", false), "{path}");
        }
        for path in [
            "C:/",
            "D:/",
            "C:/Users",
            "C:/Windows/System32",
            "C:/ProgramData/data",
        ] {
            assert!(sensitive(path, "directory", true), "{path}");
        }
        assert!(!sensitive("C:/Users/person/data", "directory", true));
        assert!(sensitive("/home/zbh", "all_local", false));
    }
    #[test]
    fn reviews_are_exact_single_use_and_expire() {
        let state = Scans::default();
        let id = Uuid::new_v4();
        state.reviews.lock().unwrap().insert(
            id,
            Review {
                binding: "expected".into(),
                issued: Instant::now(),
            },
        );
        assert!(state.consume_review(id, "changed").is_err());
        assert!(state.consume_review(id, "expected").is_ok());
        assert!(state.consume_review(id, "expected").is_err());
        state.reviews.lock().unwrap().insert(
            id,
            Review {
                binding: "expected".into(),
                issued: Instant::now() - REVIEW_TTL,
            },
        );
        assert!(state.consume_review(id, "expected").is_err());
    }
    #[test]
    fn review_binding_covers_machine_browser_scope_budgets_and_fixed_target() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("scan-review-test-{}", Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let keys = dir.join("known_hosts");
        std::fs::write(&keys, "synthetic pinned host key").unwrap();
        std::fs::set_permissions(&keys, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut target:Target=serde_json::from_value(json!({"node_id":Uuid::new_v4(),"hostname":"fixture","host":"198.51.100.2","default_user":"fixture","agent_home":"/home/fixture/.easymanage","known_hosts":keys})).unwrap();
        let mut request: Request =
            serde_json::from_value(json!({"path":"/","mode":"full","seconds":0})).unwrap();
        let original = binding(&target, &request, "browser-a").unwrap();
        assert_ne!(original, binding(&target, &request, "browser-b").unwrap());
        request.path = "/home".into();
        assert_ne!(original, binding(&target, &request, "browser-a").unwrap());
        request.path = "/".into();
        request.scope = "all_local".into();
        assert_ne!(original, binding(&target, &request, "browser-a").unwrap());
        request.scope = "directory".into();
        request.seconds = 5;
        assert_ne!(original, binding(&target, &request, "browser-a").unwrap());
        request.seconds = 0;
        request.max_entries = 1;
        assert_ne!(original, binding(&target, &request, "browser-a").unwrap());
        request.max_entries = 0;
        target.node_id = Uuid::new_v4();
        assert_ne!(original, binding(&target, &request, "browser-a").unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
