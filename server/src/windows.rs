//! Windows Agent management through explicit pwsh and SFTP. No Linux command is
//! ever sent to a Windows target. Credentials/configuration travel on SSH stdin.
use super::*;
const CONTROL: &str = include_str!("../../deploy/windows/agent-control.ps1");
pub(super) fn run_fixed(
    session: &Session,
    pwsh: &str,
    script: &str,
) -> Result<String, OperationsError> {
    session.set_timeout(50_000);
    let mut channel = crate::network::ssh_channel(session)
        .map_err(|_| OperationsError::ssh("Windows SSH 通道不可用"))?;
    channel
        .handle_extended_data(ExtendedData::Ignore)
        .map_err(|_| OperationsError::busy())?;
    channel
        .exec(&windows_invocation(pwsh, false)?)
        .map_err(|_| OperationsError::ssh("无法启动 pwsh；请安装 PowerShell 7 并核对路径"))?;
    use base64::Engine;
    let encoded =
        Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(script.as_bytes()));
    channel
        .write_all(encoded.as_bytes())
        .and_then(|()| channel.write_all(b"\n"))
        .and_then(|()| channel.send_eof().map_err(std::io::Error::from))
        .map_err(|_| OperationsError::ssh("Windows 操作传输失败"))?;
    let mut output = Vec::new();
    (&mut channel)
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut output)
        .map_err(|_| OperationsError::ssh("Windows 操作超时或通道中断"))?;
    channel
        .wait_close()
        .map_err(|_| OperationsError::ssh("Windows 操作通道关闭失败"))?;
    if output.len() > 2 * 1024 * 1024 || channel.exit_status().unwrap_or(-1) != 0 {
        return Err(OperationsError::ssh(
            "Windows 操作失败；请核对 pwsh、原节点身份、管理员权限与保留的暂存文件",
        ));
    }
    String::from_utf8(output).map_err(|_| OperationsError::ssh("Windows 输出不是 UTF-8"))
}
fn run(
    session: &Session,
    pwsh: &str,
    request: serde_json::Value,
) -> Result<String, OperationsError> {
    // Data is single-quoted with PowerShell escaping, sent over stdin, never shell arguments.
    let data = Zeroizing::new(request.to_string().replace('\'', "''"));
    let fixed = if request["action"] == "users" {
        include_str!("../../deploy/windows/manage-users.ps1")
    } else {
        CONTROL
    };
    let script = Zeroizing::new(format!(
        "& {{\n{}\n$request='{}' | ConvertFrom-Json\n{}\n}}\n",
        easymanage_runtime::shell::WINDOWS_PWSH_DISCOVERY,
        data.as_str(),
        fixed
    ));
    run_fixed(session, pwsh, &script)
}
pub(super) fn operation(
    session: &Session,
    target: &Target,
    action: &str,
    extra: serde_json::Value,
) -> Result<String, OperationsError> {
    let mut data = json!({"action":action,"base":target.agent_home.to_string_lossy(),"node_id":target.node_id,"mode":target.agent_mode.as_deref().unwrap_or("basic")});
    if let Some(extra) = extra.as_object() {
        for (key, value) in extra {
            data[key] = value.clone();
        }
    }
    run(session, &target.pwsh_path, data)
}
fn upload(session: &Session, remote: &str, bytes: &[u8]) -> Result<(), OperationsError> {
    let path = format!("/{}", remote.replace('\\', "/"));
    let sftp = session
        .sftp()
        .map_err(|_| OperationsError::ssh("Windows 需要 OpenSSH SFTP 子系统"))?;
    let mut file = sftp
        .create(FsPath::new(&path))
        .map_err(|_| OperationsError::ssh("Windows 暂存文件创建失败"))?;
    file.write_all(bytes)
        .and_then(|()| file.flush())
        .map_err(|_| OperationsError::ssh("Windows 文件上传失败"))?;
    file.close()
        .map_err(|_| OperationsError::ssh("Windows 文件上传确认失败"))
}
fn compatible_pe(bytes: &[u8], arch: &str) -> bool {
    if bytes.len() < 64 || &bytes[..2] != b"MZ" {
        return false;
    }
    let offset = u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
    let Some(header) = bytes.get(offset..offset.saturating_add(6)) else {
        return false;
    };
    &header[..4] == b"PE\0\0"
        && matches!(
            (u16::from_le_bytes([header[4], header[5]]), arch),
            (0x8664, "x64" | "x86_64") | (0xaa64, "arm64" | "aarch64")
        )
}
fn deployment_pwsh(
    info: &serde_json::Value,
    mode: DeploymentMode,
) -> Result<String, OperationsError> {
    let field = if mode == DeploymentMode::Full {
        "service_pwsh"
    } else {
        "pwsh"
    };
    if mode == DeploymentMode::Full && info[field].as_str().is_none() {
        return Err(OperationsError::bad_request(
            "Full 服务需要机器级且受保护的 PowerShell 7（MSI 或管理员目录 ZIP）；当前用户 MSIX 不用于 LocalSystem。请先安装机器级版本，未创建 Agent 目录或注册节点",
        ));
    }
    info[field]
        .as_str()
        .filter(|path| valid_windows_path(path) && valid_pwsh_path(path))
        .map(|path| path.replace('\\', "/"))
        .ok_or(OperationsError::ssh("无法确认当前 pwsh 实际可执行文件路径"))
}
fn agent_environment(
    target: &Target,
    home: &str,
    bootstrap: &str,
) -> Result<Zeroizing<String>, OperationsError> {
    let id = target.node_id;
    Ok(Zeroizing::new(format!(
        "EASYMANAGE_PWSH_PATH='{}'\nEASYMANAGE_SERVER_URL={}\nEASYMANAGE_CA_CERTIFICATE='{home}/config/ca.crt'\nEASYMANAGE_AGENT_STATE_DB='{home}/state/state.db'\nEASYMANAGE_ENROLLMENT_TOKEN={}\nEASYMANAGE_NODE_ID={id}\nEASYMANAGE_REPORT_INTERVAL_SECONDS=10\n",
        target.pwsh_path,
        target
            .server_url
            .as_deref()
            .ok_or(OperationsError::registry())?,
        bootstrap
    )))
}
pub(super) fn deploy(
    state: &OperationsState,
    id: Uuid,
    request: AddMachine,
    handle: &tokio::runtime::Handle,
    browser_token: &str,
    existing: Option<Target>,
    administrator: Option<Credentials>,
    mut session: ssh_transport::SessionLease,
    auth: ssh_transport::Auth,
) -> Result<(), OperationsError> {
    let same_admin = administrator.as_ref().is_some_and(|c| {
        c.username == auth.username
            && crate::api::constant_time_equal(&c.password, &auth.password)
            && crate::api::constant_time_equal(&c.private_key, &auth.key)
            && crate::api::constant_time_equal(&c.passphrase, &auth.passphrase)
    });
    let admin_session = if let Some(credentials) = administrator.as_ref().filter(|_| !same_admin) {
        Some(ssh_connect(
            existing.as_ref().ok_or(OperationsError::registry())?,
            credentials,
            &Cancellation::default(),
        )?)
    } else {
        None
    };
    state.machines.event(
        id,
        10,
        "running",
        "SSH 身份已验证；检查 Windows、PowerShell 7、CPU 架构及原生权限",
    );
    let info = run(
        admin_session.as_deref().unwrap_or(&session),
        &request.pwsh_path,
        json!({"action":"inspect","mode":request.deployment_mode.name()}),
    )?;
    let info: serde_json::Value = serde_json::from_str(info.trim())
        .map_err(|_| OperationsError::ssh("Windows 环境检查格式错误"))?;
    let home = info["home"]
        .as_str()
        .filter(|p| valid_windows_path(p))
        .ok_or(OperationsError::ssh("Windows Agent 目录无效"))?;
    let resolved_pwsh = deployment_pwsh(&info, request.deployment_mode)?;
    let arch = info["architecture"]
        .as_str()
        .ok_or(OperationsError::ssh("Windows CPU 架构未知"))?;
    let binary = fs::read(candidate_for(HostPlatform::Windows, Some(arch))?)
        .map_err(|_| OperationsError::registry())?;
    if !compatible_pe(&binary, arch) {
        return Err(OperationsError::bad_request(
            "Windows 候选不是匹配目标架构的 PE 可执行文件",
        ));
    }
    let hash = hex::encode(Sha256::digest(&binary));
    let ca = agent_ca(state)?;
    let key_path = state
        .registry
        .with_file_name(format!("machine-{id}.known_hosts"));
    if existing.is_none() {
        check_parent(&state.registry)?;
        let mut known = session
            .known_hosts()
            .map_err(|_| OperationsError::registry())?;
        let (key, kind) = session.host_key().ok_or(OperationsError::registry())?;
        let label = if request.port == 22 {
            request.host.to_string()
        } else {
            format!("[{}]:{}", request.host, request.port)
        };
        known
            .add(
                &label,
                key,
                "EasyManage confirmed Windows onboarding",
                kind.into(),
            )
            .map_err(|_| OperationsError::registry())?;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&key_path)
            .map_err(|_| OperationsError::registry())?;
        known
            .write_file(&key_path, KnownHostFileKind::OpenSSH)
            .map_err(|_| OperationsError::registry())?;
    }
    let mut target = existing.clone().unwrap_or(Target {
        platform: HostPlatform::Windows,
        pwsh_path: request.pwsh_path.clone(),
        architecture: Some(arch.into()),
        native_principal: info["principal"].as_str().map(str::to_owned),
        node_id: id,
        hostname: info["hostname"]
            .as_str()
            .unwrap_or(&request.display_name)
            .into(),
        host: request.host,
        port: request.port,
        default_user: request.username.clone(),
        agent_home: home.into(),
        known_hosts: key_path,
        agent_sha256: None,
        server_url: Some(request.server_url.clone()),
        ca_sha256: None,
        release_sha256: None,
        ssh_alias: None,
        disabled: false,
        connection_mode: ssh_transport::Mode::Direct,
        ssh_remote_port: None,
        agent_mode: Some(request.deployment_mode.name().into()),
    });
    if target.platform != HostPlatform::Windows || target.agent_home != PathBuf::from(home) {
        return Err(OperationsError::bad_request(
            "原机器操作系统或 Agent 路径不同，未继续部署",
        ));
    }
    target.pwsh_path = resolved_pwsh;
    target.release_sha256 = Some(hash);
    target.ca_sha256 = Some(hex::encode(Sha256::digest(&ca)));
    target.architecture = Some(arch.into());
    let stage = format!("agent.{}", Uuid::new_v4().simple());
    let stage_path = operation(
        admin_session.as_deref().unwrap_or(&session),
        &target,
        "prepare",
        json!({"stage":stage,"repair":existing.is_some()}),
    )?
    .trim()
    .to_owned();
    if stage_path != format!("{home}/staging/{stage}") {
        return Err(OperationsError::ssh("Windows 暂存目录不匹配"));
    }
    // Auto chooses the bounded SSH transport; no certificate trust store is mutated.
    let mut setup = None;
    if request.connection_mode != ssh_transport::Mode::Direct {
        let port = if ssh_transport::status(&target)["status"] == "connected" {
            target.ssh_remote_port.ok_or(OperationsError::registry())?
        } else {
            let local = checked_local_https(state, handle, &ca)?;
            let port = ssh_transport::start(&target, auth.clone(), &mut session, local)?;
            setup = Some(ssh_transport::SetupGuard::new(&target));
            port
        };
        target.connection_mode = ssh_transport::Mode::Ssh;
        target.ssh_remote_port = Some(port);
        target.server_url = Some(format!("https://127.0.0.1:{port}"));
    } else {
        target.connection_mode = ssh_transport::Mode::Direct;
        target.ssh_remote_port = None;
        target.server_url = Some(request.server_url.clone());
    }
    if existing.is_some() {
        handle.block_on(agent_service::redeploy_begin(&state.pool, id))?;
    }
    let bootstrap = Zeroizing::new(format!(
        "{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    ));
    handle.block_on(async{
        let mut tx=state.pool.begin().await.map_err(OperationsError::database)?;
        if existing.is_none(){sqlx::query("INSERT INTO nodes(id,hostname,display_name,os_name,os_version,kernel_version,architecture,cpu_model,cpu_logical_cores,agent_version,last_seen) VALUES($1,$2,$3,'Windows','','',$4,'',0,'等待 Agent',NOW()-INTERVAL '1 day')")
            .bind(id).bind(&target.hostname).bind(&request.display_name).bind(arch).execute(&mut *tx).await.map_err(OperationsError::database)?;}
        sqlx::query("INSERT INTO machine_enrollments(node_id,token_hash,expires_at,expected_user) VALUES($1,$2,NOW()+INTERVAL '10 minutes',$3) ON CONFLICT(node_id) DO UPDATE SET token_hash=EXCLUDED.token_hash,expires_at=EXCLUDED.expires_at,expected_user=EXCLUDED.expected_user")
            .bind(id).bind(crate::api::hash_token(&bootstrap)).bind(&request.username).execute(&mut *tx).await.map_err(OperationsError::database)?;
        tx.commit().await.map_err(OperationsError::database)
    })?;
    save_target(state, &target)?;
    ssh_vault::save_with_token(&state.registry, &target, &auth, "", false, browser_token)?;
    crate::read_cache::invalidate();
    let env = agent_environment(&target, home, bootstrap.as_str())?;
    let node=serde_json::to_vec(&json!({"node_id":id,"mode":request.deployment_mode.name(),"platform":"windows","principal":target.native_principal})).map_err(|_|OperationsError::registry())?;
    let mut hashes = json!({});
    for (name, bytes) in [
        ("easymanage-agent.exe", binary.as_slice()),
        ("agent.env", env.as_bytes()),
        ("ca.crt", ca.as_slice()),
        ("node.json", node.as_slice()),
    ] {
        upload(
            admin_session.as_deref().unwrap_or(&session),
            &format!("{stage_path}/{name}"),
            bytes,
        )?;
        hashes[name] = hex::encode(Sha256::digest(bytes)).into();
    }
    state.machines.event(
        id,
        60,
        "running",
        "Windows 文件已通过 SFTP 上传；验证 SHA-256，保留 SQLite 身份并启动所选 Agent 服务",
    );
    let started = Utc::now();
    operation(
        admin_session.as_deref().unwrap_or(&session),
        &target,
        "install",
        json!({"stage":stage,"hashes":hashes,"repair":existing.is_some()}),
    )?;
    state.machines.event(
        id,
        80,
        "running",
        "Windows Agent 已启动；等待原生权限、节点 ID 和本次采集时间匹配的真实心跳",
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        let valid=handle.block_on(sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM nodes n JOIN metrics_latest m ON n.id=m.node_id WHERE n.id=$1 AND n.deleted_at IS NULL AND n.last_seen>= $2 AND m.updated_at>= $2 AND (m.metrics->>'collected_at')::timestamptz>= $2 AND m.metrics->'privilege'->>'mode'=$3 AND m.metrics->'privilege'->'native'->>'platform'='windows' AND (m.metrics->'privilege'->'native'->>'administrator')::boolean=$4 AND m.metrics->'privilege'->'native'->>'principal'=$5)")
            .bind(id).bind(started).bind(request.deployment_mode.name()).bind(request.deployment_mode==DeploymentMode::Full).bind(if request.deployment_mode==DeploymentMode::Full{"S-1-5-18"}else{target.native_principal.as_deref().unwrap_or("")}).fetch_one(&state.pool)).unwrap_or(false);
        if valid {
            target.agent_sha256 = target.release_sha256.clone();
            save_target(state, &target)?;
            ssh_vault::set_enabled(
                &state.registry,
                &target,
                target.connection_mode == ssh_transport::Mode::Ssh,
            )?;
            if let Some(guard) = setup.as_mut() {
                guard.commit();
            }
            let _ = operation(
                admin_session.as_deref().unwrap_or(&session),
                &target,
                "cleanup",
                json!({"stage":stage}),
            );
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    Err(OperationsError::ssh(
        "60 秒内未确认 Windows Agent 心跳；保留节点和私有暂存文件，请检查服务日志、CA 和网络",
    ))
}
pub(super) fn update(
    session: &Session,
    target: &Target,
    binary: &FsPath,
) -> Result<(), OperationsError> {
    let bytes = fs::read(binary).map_err(|_| OperationsError::registry())?;
    let hash = hex::encode(Sha256::digest(&bytes));
    if target.release_sha256.as_deref() != Some(&hash)
        || !compatible_pe(&bytes, target.architecture.as_deref().unwrap_or("x64"))
    {
        return Err(OperationsError::bad_request(
            "Windows 更新候选摘要或架构不匹配",
        ));
    }
    let stage = format!("agent.{}", Uuid::new_v4().simple());
    let path = operation(
        session,
        target,
        "prepare",
        json!({"stage":stage,"repair":true}),
    )?
    .trim()
    .to_owned();
    if path != format!("{}/staging/{stage}", target.agent_home.to_string_lossy()) {
        return Err(OperationsError::registry());
    }
    upload(session, &format!("{path}/easymanage-agent.exe"), &bytes)?;
    operation(
        session,
        target,
        "update",
        json!({"stage":stage,"sha256":hash}),
    )?;
    let _ = operation(session, target, "cleanup", json!({"stage":stage}));
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_preflight_never_reuses_a_user_package_or_missing_service_candidate() {
        let mut info =
            json!({"pwsh":"C:/Program Files/WindowsApps/fixture/pwsh.exe","service_pwsh":null});
        assert!(deployment_pwsh(&info, DeploymentMode::Full).is_err());
        assert!(deployment_pwsh(&info, DeploymentMode::Basic).is_ok());
        info["service_pwsh"] = json!("D:/Protected/PowerShell/7/pwsh.exe");
        assert_eq!(
            deployment_pwsh(&info, DeploymentMode::Full).unwrap(),
            "D:/Protected/PowerShell/7/pwsh.exe"
        );
        for invalid in [
            "relative/pwsh.exe",
            "C:/bad;command/pwsh.exe",
            "C:/other.exe",
        ] {
            info["service_pwsh"] = json!(invalid);
            assert!(deployment_pwsh(&info, DeploymentMode::Full).is_err());
        }
    }
    #[test]
    fn generated_windows_environment_round_trips_paths_with_spaces_and_unicode() {
        let home = "D:/Managed Folder/\u{5b50}\u{76ee}\u{5f55}";
        let target:Target=serde_json::from_value(json!({"platform":"windows","pwsh_path":"C:/Program Files/PowerShell/7/pwsh.exe","node_id":Uuid::nil(),"hostname":"fixture","host":"127.0.0.1","default_user":"fixture","agent_home":home,"known_hosts":"/tmp/fixture.known_hosts","server_url":"https://127.0.0.1:8443"})).unwrap();
        let environment = agent_environment(&target, home, &"a".repeat(64)).unwrap();
        let values: std::collections::HashMap<_, _> =
            dotenvy::from_read_iter(environment.as_bytes())
                .collect::<Result<_, _>>()
                .unwrap();
        assert_eq!(values["EASYMANAGE_PWSH_PATH"], target.pwsh_path);
        assert_eq!(
            values["EASYMANAGE_AGENT_STATE_DB"],
            format!("{home}/state/state.db")
        );
        assert_eq!(
            values["EASYMANAGE_CA_CERTIFICATE"],
            format!("{home}/config/ca.crt")
        );
        assert_eq!(values["EASYMANAGE_NODE_ID"], Uuid::nil().to_string());
        assert_eq!(values["EASYMANAGE_ENROLLMENT_TOKEN"], "a".repeat(64));
    }
    #[test]
    fn pe_architecture_is_checked_and_truncated_headers_are_rejected() {
        assert!(!compatible_pe(b"MZ", "x64"));
        let mut bytes = vec![0; 128];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60] = 64;
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes[68..70].copy_from_slice(&0x8664u16.to_le_bytes());
        assert!(compatible_pe(&bytes, "x64"));
        assert!(!compatible_pe(&bytes, "arm64"));
        bytes[60..64].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(!compatible_pe(&bytes, "x64"));
    }
}

pub(super) fn promote(
    state: &OperationsState,
    target: &Target,
    credentials: Credentials,
    cancellation: &Cancellation,
) -> Result<(), OperationsError> {
    let session = ssh_connect(target, &credentials, cancellation)?;
    let info = run(
        &session,
        &target.pwsh_path,
        json!({"action":"inspect","mode":"full"}),
    )?;
    let info: serde_json::Value = serde_json::from_str(info.trim())
        .map_err(|_| OperationsError::ssh("Windows 管理员环境检查失败"))?;
    let home = info["home"]
        .as_str()
        .filter(|p| valid_windows_path(p))
        .ok_or(OperationsError::registry())?;
    let mut full = target.clone();
    full.agent_home = home.into();
    full.agent_mode = Some("full".into());
    full.pwsh_path = deployment_pwsh(&info, DeploymentMode::Full)?;
    let binary = fs::read(candidate_for(
        HostPlatform::Windows,
        target.architecture.as_deref(),
    )?)
    .map_err(|_| OperationsError::registry())?;
    let hash = hex::encode(Sha256::digest(&binary));
    let ca = agent_ca(state)?;
    if target.ca_sha256.as_deref() != Some(hex::encode(Sha256::digest(&ca)).as_str()) {
        return Err(OperationsError::bad_request(
            "原 Agent CA 与平台不一致，未迁移身份",
        ));
    }
    let stage = format!("agent.{}", Uuid::new_v4().simple());
    let path = operation(
        &session,
        &full,
        "prepare",
        json!({"stage":stage,"repair":false,"promotion":true}),
    )?
    .trim()
    .to_owned();
    if path != format!("{home}/staging/{stage}") {
        return Err(OperationsError::registry());
    }
    upload(&session, &format!("{path}/easymanage-agent.exe"), &binary)?;
    operation(
        &session,
        &full,
        "promote",
        json!({"stage":stage,"source_base":target.agent_home.to_string_lossy(),"source_principal":target.native_principal,"sha256":hash,"ca_sha256":target.ca_sha256,"pwsh":full.pwsh_path}),
    )?;
    full.agent_sha256 = Some(hash.clone());
    full.release_sha256 = Some(hash);
    save_target(state, &full)?;
    Ok(())
}
