//! Persistent, same-origin collection settings. No service/SSH/sudo operations.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
};
use easymanage_protocol::{CollectionPolicy, collection::COLLECTION_ITEMS};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::sync::Arc;
use tokio::sync::{Mutex, watch};

#[derive(Clone)]
pub struct CollectionSettings {
    pub sender: watch::Sender<CollectionPolicy>,
    save_lock: Arc<Mutex<()>>,
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
        Self { sender, save_lock: Arc::new(Mutex::new(())) }
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
        self.sender.send_replace(policy);
        Ok(())
    }
    pub fn current(&self) -> CollectionPolicy {
        self.sender.borrow().clone()
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
            "applied_policy":row.try_get::<Option<Value>,_>("applied_policy")? }))
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
