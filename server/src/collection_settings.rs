//! Persistent, same-origin collection settings. No service/SSH/sudo operations.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use easymanage_protocol::{CollectionPolicy, collection::COLLECTION_ITEMS};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::{
    collections::HashSet,
    sync::{Arc, RwLock},
};
use tokio::sync::{Mutex, watch};
use uuid::Uuid;

#[derive(Clone)]
pub struct CollectionSettings {
    pub sender: watch::Sender<CollectionPolicy>,
    save_lock: Arc<Mutex<()>>,
    disabled_users: Arc<RwLock<HashSet<Uuid>>>,
    disabled_permissions: Arc<RwLock<HashSet<Uuid>>>,
    disabled_proxy: Arc<RwLock<HashSet<Uuid>>>,
}

impl CollectionSettings {
    pub fn defaults(report_seconds: u64) -> Self {
        let report_seconds = report_seconds.clamp(5, 30);
        let default = CollectionPolicy {
            revision: 1,
            report_seconds,
            gpu_seconds: report_seconds,
            ..Default::default()
        };
        let (sender, _) = watch::channel(default);
        Self {
            sender,
            save_lock: Arc::new(Mutex::new(())),
            disabled_users: Arc::default(),
            disabled_permissions: Arc::default(),
            disabled_proxy: Arc::default(),
        }
    }

    pub async fn initialize(&self, pool: &PgPool) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO collection_settings (singleton, policy) VALUES (TRUE, $1) ON CONFLICT (singleton) DO NOTHING")
            .bind(json!(self.current())).execute(pool).await?;
        let value: Value =
            sqlx::query_scalar("SELECT policy FROM collection_settings WHERE singleton=TRUE")
                .fetch_one(pool)
                .await?;
        let policy: CollectionPolicy = serde_json::from_value(value)?;
        policy.validate().map_err(anyhow::Error::msg)?;
        let disabled: Vec<Uuid> =
            sqlx::query_scalar("SELECT node_id FROM node_user_collection WHERE NOT enabled")
                .fetch_all(pool)
                .await?;
        *self
            .disabled_users
            .write()
            .unwrap_or_else(|e| e.into_inner()) = disabled.into_iter().collect();
        self.sender.send_replace(policy);
        let disabled: Vec<Uuid> =
            sqlx::query_scalar("SELECT node_id FROM node_permission_collection WHERE NOT enabled")
                .fetch_all(pool)
                .await?;
        *self
            .disabled_permissions
            .write()
            .unwrap_or_else(|e| e.into_inner()) = disabled.into_iter().collect();
        let disabled: Vec<Uuid> =
            sqlx::query_scalar("SELECT node_id FROM node_proxy_collection WHERE NOT enabled")
                .fetch_all(pool)
                .await?;
        *self
            .disabled_proxy
            .write()
            .unwrap_or_else(|e| e.into_inner()) = disabled.into_iter().collect();
        Ok(())
    }
    pub fn current(&self) -> CollectionPolicy {
        self.sender.borrow().clone()
    }
    pub fn users_enabled(&self, id: Uuid) -> bool {
        !self
            .disabled_users
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&id)
    }
    pub fn permissions_enabled(&self, id: Uuid) -> bool {
        !self
            .disabled_permissions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&id)
    }
    pub fn for_node(
        &self,
        id: Uuid,
        supports_users: bool,
        supports_permissions: bool,
    ) -> CollectionPolicy {
        let mut policy = self.current();
        policy.users_enabled = supports_users.then(|| self.users_enabled(id));
        policy.permissions_enabled = supports_permissions.then(|| self.permissions_enabled(id));
        policy
    }

    pub fn proxy_enabled(&self, id: Uuid) -> bool {
        !self
            .disabled_proxy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&id)
    }

    pub fn for_node_with_proxy(
        &self,
        id: Uuid,
        users: bool,
        permissions: bool,
        proxy: bool,
    ) -> CollectionPolicy {
        let mut policy = self.for_node(id, users, permissions);
        policy.proxy_enabled = proxy.then(|| self.proxy_enabled(id));
        policy
    }
}

#[derive(Clone)]
struct SettingsState {
    pool: PgPool,
    settings: CollectionSettings,
}

pub fn router(pool: PgPool, settings: CollectionSettings) -> Router {
    Router::new()
        .route("/api/v1/collection-settings", get(list).post(save))
        .route("/api/v1/nodes/{id}/user-collection", post(save_users))
        .route("/api/v1/nodes/{id}/proxy-collection", post(save_proxy))
        .route(
            "/api/v1/nodes/{id}/permission-collection",
            post(save_permissions),
        )
        .layer(DefaultBodyLimit::max(4096))
        .with_state(SettingsState { pool, settings })
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"message":message}))).into_response()
}
fn database_error(cause: sqlx::Error) -> Response {
    tracing::warn!(%cause, "collection settings storage failed");
    error(StatusCode::SERVICE_UNAVAILABLE, "采集设置存储暂不可用")
}

async fn list(State(state): State<SettingsState>) -> Result<Json<Value>, Response> {
    let policy = state.settings.current();
    let items: Vec<_> = COLLECTION_ITEMS.iter().map(|(key,title,description,min,max,default)|
        json!({"key":key,"title":title,"description":description,"minimum":min,"maximum":max,"default_seconds":default})).collect();
    let nodes = sqlx::query("SELECT n.id,COALESCE(NULLIF(n.display_name,''),n.hostname) AS hostname,n.last_seen,m.metrics->'collection_policy' AS applied_policy FROM nodes n LEFT JOIN metrics_latest m ON m.node_id=n.id WHERE n.deleted_at IS NULL ORDER BY n.hostname")
        .fetch_all(&state.pool).await.map_err(database_error)?;
    let agents: Vec<_> = nodes.into_iter().map(|row| -> Result<Value, sqlx::Error> {
        Ok(json!({ "node_id":row.try_get::<uuid::Uuid,_>("id")?,"hostname":row.try_get::<String,_>("hostname")?,
            "last_seen":row.try_get::<chrono::DateTime<chrono::Utc>,_>("last_seen")?,
            "applied_policy":row.try_get::<Option<Value>,_>("applied_policy")?,
            "users_enabled":state.settings.users_enabled(row.try_get("id")?),
            "permissions_enabled":state.settings.permissions_enabled(row.try_get("id")?),
            "proxy_enabled":state.settings.proxy_enabled(row.try_get("id")?) }))
    }).collect::<Result<_,_>>().map_err(database_error)?;
    Ok(Json(json!({"policy":policy,"items":items,"agents":agents})))
}

async fn save(
    State(state): State<SettingsState>,
    headers: HeaderMap,
    uri: Uri,
    Json(mut policy): Json<CollectionPolicy>,
) -> Result<Json<Value>, Response> {
    crate::operations::validate_origin(&headers, &uri).map_err(IntoResponse::into_response)?;
    if policy.users_enabled.is_some()
        || policy.permissions_enabled.is_some()
        || policy.proxy_enabled.is_some()
    {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "用户、权限检查和代理采集开关必须按机器单独设置",
        ));
    }
    policy
        .validate()
        .map_err(|message| error(StatusCode::BAD_REQUEST, &message))?;
    let _guard = state.settings.save_lock.lock().await;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let before: Value = sqlx::query_scalar(
        "SELECT policy FROM collection_settings WHERE singleton=TRUE FOR UPDATE",
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    if before.get("revision").and_then(Value::as_u64) != Some(policy.revision) {
        return Err(error(
            StatusCode::CONFLICT,
            "采集设置已被其他页面修改，请重新加载后保存",
        ));
    }
    policy.revision = policy
        .revision
        .checked_add(1)
        .filter(|v| *v <= i64::MAX as u64)
        .ok_or_else(|| error(StatusCode::CONFLICT, "配置版本已达上限"))?;
    sqlx::query("UPDATE collection_settings SET policy=$1,updated_at=NOW() WHERE singleton=TRUE")
        .bind(json!(policy))
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    sqlx::query("INSERT INTO audit_logs (actor_type, actor_id, action, target_type, target_id, details) VALUES ('browser', 'unverified-intranet-operator', 'collection.settings.update', 'platform', 'global', $1)")
        .bind(json!({"before":before,"after":policy})).execute(&mut *tx).await.map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    state.settings.sender.send_replace(policy.clone());
    Ok(Json(
        json!({"policy":policy,"message":"设置已保存；平台立即调整采样，支持此功能的 Agent 在下一次成功完整上报后应用"}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeCollectionChange {
    enabled: bool,
    expected_enabled: bool,
}

async fn save_users(
    State(state): State<SettingsState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    uri: Uri,
    Json(change): Json<NodeCollectionChange>,
) -> Result<Json<Value>, Response> {
    save_node(state, id, headers, uri, change, NodeCollectionKind::Users).await
}

async fn save_permissions(
    State(state): State<SettingsState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    uri: Uri,
    Json(change): Json<NodeCollectionChange>,
) -> Result<Json<Value>, Response> {
    save_node(
        state,
        id,
        headers,
        uri,
        change,
        NodeCollectionKind::Permissions,
    )
    .await
}

#[derive(Clone, Copy)]
enum NodeCollectionKind {
    Users,
    Permissions,
    Proxy,
}

async fn save_proxy(
    State(state): State<SettingsState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    uri: Uri,
    Json(change): Json<NodeCollectionChange>,
) -> Result<Json<Value>, Response> {
    save_node(state, id, headers, uri, change, NodeCollectionKind::Proxy).await
}

async fn save_node(
    state: SettingsState,
    id: Uuid,
    headers: HeaderMap,
    uri: Uri,
    change: NodeCollectionChange,
    kind: NodeCollectionKind,
) -> Result<Json<Value>, Response> {
    crate::operations::validate_origin(&headers, &uri).map_err(IntoResponse::into_response)?;
    let _guard = state.settings.save_lock.lock().await;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    // Lock the node even when no setting row exists yet, and exclude deleted nodes.
    let exists: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM nodes WHERE id=$1 AND deleted_at IS NULL FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(database_error)?;
    if exists.is_none() {
        return Err(error(StatusCode::NOT_FOUND, "机器不存在或已删除"));
    }
    let before: bool = sqlx::query_scalar(match kind {
        NodeCollectionKind::Permissions => {
            "SELECT enabled FROM node_permission_collection WHERE node_id=$1"
        }
        NodeCollectionKind::Users => "SELECT enabled FROM node_user_collection WHERE node_id=$1",
        NodeCollectionKind::Proxy => "SELECT enabled FROM node_proxy_collection WHERE node_id=$1",
    })
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(database_error)?
    .unwrap_or(true);
    if before != change.expected_enabled {
        return Err(error(
            StatusCode::CONFLICT,
            "该机器的采集开关已被其他页面修改，请刷新后重试",
        ));
    }
    sqlx::query(match kind {
        NodeCollectionKind::Permissions => "INSERT INTO node_permission_collection(node_id,enabled) VALUES($1,$2) ON CONFLICT(node_id) DO UPDATE SET enabled=EXCLUDED.enabled,updated_at=NOW()",
        NodeCollectionKind::Users => "INSERT INTO node_user_collection(node_id,enabled) VALUES($1,$2) ON CONFLICT(node_id) DO UPDATE SET enabled=EXCLUDED.enabled,updated_at=NOW()",
        NodeCollectionKind::Proxy => "INSERT INTO node_proxy_collection(node_id,enabled) VALUES($1,$2) ON CONFLICT(node_id) DO UPDATE SET enabled=EXCLUDED.enabled,updated_at=NOW()"})
        .bind(id).bind(change.enabled).execute(&mut *tx).await.map_err(database_error)?;
    sqlx::query("INSERT INTO audit_logs(actor_type,actor_id,action,target_type,target_id,details) VALUES('browser','unverified-intranet-operator',$1,'node',$2,$3)")
        .bind(match kind {NodeCollectionKind::Permissions=>"collection.permissions.update",NodeCollectionKind::Users=>"collection.users.update",NodeCollectionKind::Proxy=>"collection.proxy.update"})
        .bind(id.to_string()).bind(json!({"before":before,"after":change.enabled})).execute(&mut *tx).await.map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    let mut disabled = match kind {
        NodeCollectionKind::Permissions => &state.settings.disabled_permissions,
        NodeCollectionKind::Users => &state.settings.disabled_users,
        NodeCollectionKind::Proxy => &state.settings.disabled_proxy,
    }
    .write()
    .unwrap_or_else(|e| e.into_inner());
    if change.enabled {
        disabled.remove(&id);
    } else {
        disabled.insert(id);
    }
    Ok(Json(
        json!({"enabled":change.enabled,"message":"设置已保存，等待 Agent 下次完整上报应用；旧版 Agent 需更新"}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proxy_switch_is_independent_and_omitted_for_legacy_report_and_control_peers() {
        let settings = CollectionSettings::defaults(10);
        let node = Uuid::new_v4();
        let other = Uuid::new_v4();
        assert_eq!(
            settings
                .for_node_with_proxy(node, true, true, true)
                .proxy_enabled,
            Some(true)
        );
        settings.disabled_proxy.write().unwrap().insert(node);
        let policy = settings.for_node_with_proxy(node, true, true, true);
        assert_eq!(policy.proxy_enabled, Some(false));
        assert_eq!(policy.users_enabled, Some(true));
        assert_eq!(policy.permissions_enabled, Some(true));
        assert_eq!(
            settings
                .for_node_with_proxy(other, true, true, true)
                .proxy_enabled,
            Some(true)
        );
        assert!(
            json!(settings.for_node_with_proxy(node, true, true, false))
                .get("proxy_enabled")
                .is_none()
        );
        assert!(json!(settings.current()).get("proxy_enabled").is_none());
        let old: easymanage_protocol::AgentControlRequest =
            serde_json::from_value(json!({"node_id":node,"applied":{"revision":0,"paused":false}}))
                .unwrap();
        assert!(!old.supports_proxy_collection);
    }
    #[test]
    fn user_switch_is_node_scoped_and_legacy_safe() {
        let legacy = json!(CollectionPolicy::default());
        assert!(legacy.get("users_enabled").is_none());
        assert_eq!(
            serde_json::from_value::<CollectionPolicy>(legacy)
                .unwrap()
                .users_enabled,
            None
        );
        let settings = CollectionSettings::defaults(10);
        let first = Uuid::new_v4();
        let other = Uuid::new_v4();
        assert_eq!(
            settings.for_node(first, true, false).users_enabled,
            Some(true)
        );
        settings.disabled_users.write().unwrap().insert(first);
        assert_eq!(
            settings.for_node(first, true, false).users_enabled,
            Some(false)
        );
        assert_eq!(
            serde_json::from_value::<CollectionPolicy>(json!(
                settings.for_node(first, true, false)
            ))
            .unwrap()
            .users_enabled,
            Some(false)
        );
        assert_eq!(
            settings.for_node(other, true, false).users_enabled,
            Some(true)
        );
        assert!(
            !json!(settings.for_node(first, false, false))
                .as_object()
                .unwrap()
                .contains_key("users_enabled")
        );
        assert_eq!(settings.current().users_enabled, None);
    }

    #[test]
    fn permissions_switch_is_independent_of_users_and_legacy_peers() {
        let settings = CollectionSettings::defaults(10);
        let node = Uuid::new_v4();
        let other = Uuid::new_v4();
        assert_eq!(
            settings.for_node(node, true, true).permissions_enabled,
            Some(true)
        );
        settings.disabled_permissions.write().unwrap().insert(node);
        let policy = settings.for_node(node, true, true);
        assert_eq!(policy.permissions_enabled, Some(false));
        assert_eq!(policy.users_enabled, Some(true));
        assert_eq!(
            settings.for_node(other, true, true).permissions_enabled,
            Some(true)
        );
        assert!(
            json!(settings.for_node(node, true, false))
                .get("permissions_enabled")
                .is_none()
        );
        assert_eq!(settings.current().permissions_enabled, None);
    }
}
