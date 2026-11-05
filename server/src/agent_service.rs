//! Explicit process shutdown, independent of cooperative metrics pause.
use super::*;

const STOP_SCRIPT: &str = include_str!("../../deploy/linux/operations/stop-agent-service.py");

fn marker(registry:&FsPath,id:Uuid)->Result<PathBuf,OperationsError>{
    Ok(registry.parent().ok_or(OperationsError::registry())?.join("agent-service-blocks").join(id.to_string()))
}

pub(super) fn emergency_allowed(registry:&FsPath,id:Uuid)->Result<(),OperationsError>{
    use std::os::unix::fs::{MetadataExt,PermissionsExt};
    let path=marker(registry,id)?;
    match std::fs::symlink_metadata(path.parent().ok_or(OperationsError::registry())?){
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok(()),
        Err(_)=>return Err(OperationsError::registry()),
        Ok(_)=>machines::check_parent(&path)?,
    }
    match std::fs::symlink_metadata(&path){
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Ok(()),
        Err(_)=>Err(OperationsError::registry()),
        Ok(meta)=>{
            if !meta.is_file()||meta.permissions().mode()&0o077!=0||meta.uid()!=std::fs::metadata("/proc/self").map_err(|_|OperationsError::registry())?.uid(){return Err(OperationsError::registry());}
            Err(OperationsError(StatusCode::CONFLICT,"Agent 已停止或停服状态未确认；只能重新部署后继续，应急重启不会绕过此状态"))
        }
    }
}

fn set_marker(registry:&FsPath,id:Uuid,blocked:bool)->Result<(),OperationsError>{
    use std::os::unix::fs::{DirBuilderExt,OpenOptionsExt};
    let path=marker(registry,id)?;
    if blocked{
        machines::check_parent(registry)?;
        let directory=path.parent().ok_or(OperationsError::registry())?;
        match std::fs::DirBuilder::new().mode(0o700).create(directory){
            Ok(())=>{},Err(error) if error.kind()==std::io::ErrorKind::AlreadyExists=>{},Err(_)=>return Err(OperationsError::registry()),
        }
        machines::check_parent(&path)?;
        match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path){
            Ok(file)=>file.sync_all().map_err(|_|OperationsError::registry())?,
            Err(error) if error.kind()==std::io::ErrorKind::AlreadyExists=>{
                let result=emergency_allowed(registry,id);
                if !matches!(result,Err(OperationsError(StatusCode::CONFLICT,_))){return Err(OperationsError::registry());}
            },Err(_)=>return Err(OperationsError::registry()),
        }
        std::fs::File::open(directory).and_then(|file|file.sync_all()).map_err(|_|OperationsError::registry())?;
    }else{
        match emergency_allowed(registry,id){
            Ok(())=>return Ok(()),
            Err(OperationsError(StatusCode::CONFLICT,_))=>{},Err(error)=>return Err(error),
        }
        std::fs::remove_file(&path).map_err(|_|OperationsError::registry())?;
        std::fs::File::open(path.parent().ok_or(OperationsError::registry())?).and_then(|file|file.sync_all()).map_err(|_|OperationsError::registry())?;
    }
    Ok(())
}

pub(super) fn router() -> Router<OperationsState> {
    Router::new().route("/api/v1/machines/{node_id}/agent/stop", post(stop))
}

pub(super) fn initialize(state: OperationsState) {
    tokio::spawn(async move {
        crate::database::wait_for_initialization(&state.initialized).await;
        // Never repeat a remote side effect after a platform crash.
        if sqlx::query("UPDATE agent_service_controls SET state='unknown',last_error='平台重启中断了服务操作；实际进程状态未确认，请重新核验或重新部署',updated_at=NOW() WHERE state IN ('stopping','redeploying')")
            .execute(&state.pool).await.is_err() {
            tracing::warn!("interrupted Agent service operations could not be marked unknown");
        }
    });
}

pub(super) async fn require_running(pool: &PgPool, id: Uuid) -> Result<(), OperationsError> {
    let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM agent_service_controls WHERE node_id=$1 AND (desired_stopped OR state NOT IN ('running')))")
        .bind(id).fetch_one(pool).await.map_err(OperationsError::database)?;
    if blocked {
        return Err(OperationsError(StatusCode::CONFLICT,"Agent 已停止或停服状态未确认；继续使用需重新部署 Agent，不能仅恢复监控或重启"));
    }
    Ok(())
}

pub(super) async fn stopped(pool: &PgPool, id: Uuid) -> Result<bool, OperationsError> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM agent_service_controls WHERE node_id=$1 AND (desired_stopped OR state='unknown'))")
        .bind(id).fetch_one(pool).await.map_err(OperationsError::database)
}

pub(super) async fn redeploy_begin(pool: &PgPool, id: Uuid) -> Result<(), OperationsError> {
    let mut tx=pool.begin().await.map_err(OperationsError::database)?;
    let changed=sqlx::query("UPDATE agent_service_controls SET desired_stopped=FALSE,state='redeploying',revision=revision+1,last_error=NULL,updated_at=NOW() WHERE node_id=$1 AND state<>'running'")
        .bind(id).execute(&mut *tx).await.map_err(OperationsError::database)?.rows_affected();
    if changed>0 {
        sqlx::query("UPDATE nodes SET monitoring_paused=FALSE,monitoring_revision=monitoring_revision+1,updated_at=NOW() WHERE id=$1")
            .bind(id).execute(&mut *tx).await.map_err(OperationsError::database)?;
    }
    tx.commit().await.map_err(OperationsError::database)?;
    crate::read_cache::invalidate();
    Ok(())
}

pub(super) async fn redeploy_finish(state: &OperationsState, id: Uuid, success: bool) -> Result<(), OperationsError> {
    if success{set_marker(&state.registry,id,false)?;}
    sqlx::query("UPDATE agent_service_controls SET state=$2,last_error=$3,updated_at=NOW() WHERE node_id=$1 AND state='redeploying'")
        .bind(id).bind(if success{"running"}else{"unknown"})
        .bind(if success{None}else{Some("重新部署未确认完成；请检查实际服务状态，或再次重新部署")})
        .execute(&state.pool).await.map_err(OperationsError::database)?;
    crate::read_cache::invalidate();
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StopRequest {
    confirmation: Uuid,
    expected_revision: i64,
    #[serde(default)]
    credentials: Credentials,
}

async fn stop(State(state): State<OperationsState>, Path(id): Path<Uuid>, uri: Uri, headers: HeaderMap,
    payload: Result<Json<StopRequest>,axum::extract::rejection::JsonRejection>) -> Result<Response,OperationsError> {
    validate_origin(&headers,&uri)?;
    let Json(request)=payload.map_err(|_|OperationsError::bad_request("停止 Agent 请求格式错误；凭据不会回显"))?;
    if request.confirmation!=id || !(0..9_007_199_254_740_991).contains(&request.expected_revision) {
        return Err(OperationsError::bad_request("请明确确认原机器 ID 和服务状态版本"));
    }
    if state.machines.running(id)? { return Err(OperationsError::busy()); }
    let (target,lease)=target_and_lease(&state,id,true)?;
    let row=sqlx::query("SELECT m.metrics->'privilege'->>'mode' AS mode,s.revision AS service_revision FROM nodes n LEFT JOIN metrics_latest m ON m.node_id=n.id LEFT JOIN agent_service_controls s ON s.node_id=n.id WHERE n.id=$1 AND n.deleted_at IS NULL")
        .bind(id).fetch_optional(&state.pool).await.map_err(OperationsError::database)?
        .ok_or(OperationsError(StatusCode::NOT_FOUND,"机器不存在或已软删除"))?;
    if row.get::<Option<i64>,_>("service_revision").unwrap_or(0)!=request.expected_revision {
        return Err(OperationsError(StatusCode::CONFLICT,"Agent 服务状态已变化，请刷新后重试"));
    }
    let observed: Option<String>=row.get("mode");
    let full=target.default_user=="root" || target.agent_mode.as_deref()==Some("full") || observed.as_deref()==Some("full");
    let mut credentials=request.credentials;
    if full {
        credentials=resolve_credentials(&state,&target,&headers,credentials)?;
    } else {
        if credentials.use_saved {
            if !credentials.username.is_empty() || !credentials.password.is_empty() || !credentials.private_key.is_empty() || !credentials.passphrase.is_empty() || !credentials.sudo_password.is_empty() {
                return Err(OperationsError::bad_request("使用已保存凭据时不能同时提交新凭据"));
            }
            let saved=ssh_vault::load(&state.registry,&target)?;
            if !ssh_vault::authorized(&saved,&target,&headers) {
                return Err(OperationsError(StatusCode::UNAUTHORIZED,"当前浏览器没有使用此机器已保存 SSH 凭据的授权，请鉴权一次"));
            }
            credentials=Credentials{username:saved.username,password:saved.password,private_key:saved.key,passphrase:saved.passphrase,..Default::default()};
        }
        validate_credentials(&credentials)?;
        if credentials.username!=target.default_user || !credentials.sudo_password.is_empty() {
            return Err(OperationsError::bad_request("Basic Agent 必须由原 SSH 用户停止，不调用 sudo 或更换账户"));
        }
    }
    let browser_token=if !full { ssh_vault::load(&state.registry,&target).ok().and_then(|saved|ssh_vault::browser_token(&saved,&target,&headers)) } else { None };
    let actor=credentials.username.clone();
    let expected=request.expected_revision;
    let pool=state.pool.clone();
    let registry=state.registry.clone();
    // A detached task owns both the lease and completion write even if the
    // browser closes. SSH authentication precedes all durable stop intent.
    let task=tokio::spawn(async move {
        let handle=tokio::runtime::Handle::current();
        let worker_pool=pool.clone();
        let worker=tokio::task::spawn_blocking(move || {
            let _lease=lease;
            let cancellation=Cancellation::default();
            let mut effective=target.clone();
            if effective.connection_mode==ssh_transport::Mode::Ssh && ssh_transport::status(&target)["status"]!="connected" {
                effective.connection_mode=ssh_transport::Mode::Direct;
            }
            // Prove the caller's credential before disabling automatic SSH.
            let session=ssh_connect(&effective,&credentials,&cancellation)?;
            session.set_timeout(65_000);
            let revision=handle.block_on(async {
                let mut tx=worker_pool.begin().await.map_err(OperationsError::database)?;
                sqlx::query("SET LOCAL lock_timeout='2s'").execute(&mut *tx).await.map_err(OperationsError::database)?;
                let changed:Option<i64>=sqlx::query_scalar("INSERT INTO agent_service_controls(node_id,desired_stopped,state,revision) SELECT $1,TRUE,'stopping',1 WHERE $2=0 ON CONFLICT(node_id) DO UPDATE SET desired_stopped=TRUE,state='stopping',revision=agent_service_controls.revision+1,last_error=NULL,updated_at=NOW() WHERE agent_service_controls.revision=$2 RETURNING revision")
                    .bind(id).bind(request.expected_revision).fetch_optional(&mut *tx).await.map_err(OperationsError::database)?;
                // The insert SELECT must also allow updates when revision > 0.
                let revision=if let Some(value)=changed{value}else{
                    sqlx::query_scalar("UPDATE agent_service_controls SET desired_stopped=TRUE,state='stopping',revision=revision+1,last_error=NULL,updated_at=NOW() WHERE node_id=$1 AND revision=$2 RETURNING revision")
                        .bind(id).bind(request.expected_revision).fetch_optional(&mut *tx).await.map_err(OperationsError::database)?
                        .ok_or(OperationsError(StatusCode::CONFLICT,"Agent 服务状态已变化，请刷新后重试"))?
                };
                sqlx::query("INSERT INTO audit_logs(actor_type,actor_id,action,target_type,target_id,details) VALUES('browser',$1,'agent.stop.request','node',$2,$3)")
                    .bind(&actor).bind(id.to_string()).bind(json!({"revision":revision})).execute(&mut *tx).await.map_err(OperationsError::database)?;
                tx.commit().await.map_err(OperationsError::database)?;
                Ok::<_,OperationsError>(revision)
            })?;
            crate::read_cache::invalidate();
            let operation=(|| {
                // The recovery endpoint must remain independent of PostgreSQL,
                // including while a stop is in flight or its result is unknown.
                set_marker(&registry,id,true)?;
                let invocation=format!("/usr/bin/timeout --kill-after=3s 60s /usr/bin/python3 -I -c {} {} {} {} {}",shell_quote(STOP_SCRIPT),if full{"full"}else{"basic"},
                    shell_quote(&id.to_string()),shell_quote(&target.agent_home.to_string_lossy()),shell_quote(&target.default_user));
                if full {
                    run_privileged_on(&session,credentials,&invocation)?;
                    drop(session);
                } else {
                    let output=machines::command(&session,&invocation)?;
                    if output.trim()!="AGENT_STOPPED_FILES_RETAINED" { return Err(OperationsError::ssh("远端未确认 Agent 完全停止")); }
                    let auth=ssh_transport::Auth{username:credentials.username,password:credentials.password,key:credentials.private_key,passphrase:credentials.passphrase};
                    // Keep only an encrypted credential for a later redeploy.
                    let saved=if let Some(token)=browser_token {
                        ssh_vault::save_with_token(&registry,&target,&auth,"",false,&token).map(|()|ssh_vault::cookie(&target,&token))
                    } else { ssh_vault::save(&registry,&target,&auth,"",false).map(|token|ssh_vault::cookie(&target,&token)) };
                    drop(session);
                    let disconnected=ssh_transport::stop(&target);
                    let cookie=saved?;disconnected?;
                    return Ok(Some(cookie));
                }
                let disabled=if ssh_vault::exists(&registry,&target) { ssh_vault::set_enabled(&registry,&target,false) } else { Ok(()) };
                let disconnected=ssh_transport::stop(&target);
                disabled?;disconnected?;
                Ok::<_,OperationsError>(None)
            })();
            let success=operation.is_ok();
            let stored=handle.block_on(async {
                let mut tx=worker_pool.begin().await.map_err(OperationsError::database)?;
                sqlx::query("UPDATE agent_service_controls SET state=$3,last_error=$4,updated_at=NOW() WHERE node_id=$1 AND revision=$2")
                    .bind(id).bind(revision).bind(if success{"stopped"}else{"unknown"})
                    .bind(if success{None}else{Some("停止操作未完全确认；Agent 可能仍运行，请核验或重试停止。文件和身份保留")})
                    .execute(&mut *tx).await.map_err(OperationsError::database)?;
                sqlx::query("INSERT INTO audit_logs(actor_type,actor_id,action,target_type,target_id,details) VALUES('browser',$1,'agent.stop.result','node',$2,$3)")
                    .bind(&actor).bind(id.to_string()).bind(json!({"revision":revision,"success":success})).execute(&mut *tx).await.map_err(OperationsError::database)?;
                tx.commit().await.map_err(OperationsError::database)
            });
            crate::read_cache::invalidate();
            stored?;
            operation
        });
        match worker.await {
            Ok(result)=>result,
            Err(_)=>{
                let _=sqlx::query("UPDATE agent_service_controls SET state='unknown',last_error='停止任务异常中断；实际服务状态未确认',updated_at=NOW() WHERE node_id=$1 AND revision=$2 AND state='stopping'")
                    .bind(id).bind(expected+1).execute(&pool).await;
                crate::read_cache::invalidate();
                Err(OperationsError::ssh("停止任务中断；进程状态未确认，请重新核验"))
            }
        }
    });
    let cookie=task.await.map_err(|_|OperationsError::ssh("停止任务未确认；请刷新机器状态"))??;
    let mut response=Json(json!({"message":"Agent 进程、心跳和自动拉起已停止，SSH 监控长连接已关闭；文件、配置、身份和历史保留。继续使用请重新部署 Agent。"})).into_response();
    if let Some(cookie)=cookie { response.headers_mut().insert(axum::http::header::SET_COOKIE,cookie.parse().map_err(|_|OperationsError::busy())?); }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink,DirBuilderExt};

    #[test]
    fn recovery_stop_marker_is_durable_scoped_and_cannot_overwrite_a_symlink() {
        let root=std::env::temp_dir().join(format!("agent-stop-marker-{}",Uuid::new_v4()));
        std::fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let registry=root.join("targets.json");let id=Uuid::new_v4();let other=Uuid::new_v4();
        assert!(emergency_allowed(&registry,id).is_ok());
        set_marker(&registry,id,true).unwrap();
        assert!(matches!(emergency_allowed(&registry,id),Err(OperationsError(StatusCode::CONFLICT,_))));
        assert!(emergency_allowed(&registry,other).is_ok());
        set_marker(&registry,id,true).unwrap();
        set_marker(&registry,id,false).unwrap();
        assert!(emergency_allowed(&registry,id).is_ok());
        let sentinel=root.join("keep");std::fs::write(&sentinel,b"retained").unwrap();
        symlink(&sentinel,marker(&registry,id).unwrap()).unwrap();
        assert!(set_marker(&registry,id,true).is_err());
        assert!(set_marker(&registry,id,false).is_err());
        assert_eq!(std::fs::read(&sentinel).unwrap(),b"retained");
        std::fs::remove_dir_all(root).unwrap();
    }
}
