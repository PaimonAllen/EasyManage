//! Keep HTTPS and authenticated recovery available while PostgreSQL is down.
use axum::{Json, extract::{Request, State}, http::StatusCode, middleware::Next, response::{IntoResponse, Response}};
use serde_json::json;
use sqlx::PgPool;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

pub type Initialized = Arc<AtomicBool>;

pub async fn wait_for_initialization(ready: &Initialized) {
    while !ready.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub fn initialize(pool: PgPool, settings: crate::collection_settings::CollectionSettings) -> Initialized {
    let ready = Arc::new(AtomicBool::new(false));
    let status = ready.clone();
    tokio::spawn(async move {
        loop {
            let result = async {
                sqlx::migrate!("../migrations").run(&pool).await?;
                settings.initialize(&pool).await?;
                Ok::<_, anyhow::Error>(())
            }.await;
            match result {
                Ok(()) => {
                    status.store(true, Ordering::Release);
                    tracing::info!("database initialized; monitoring APIs enabled");
                    break;
                }
                Err(error) => tracing::warn!(%error, "database unavailable; HTTPS recovery remains available, retrying in 10 seconds"),
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
    ready
}

pub async fn guard(State(ready): State<Initialized>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let recovery = !path.starts_with("/api/") || matches!(path,
        "/api/v1/health/live" | "/api/v1/platform" | "/api/v1/operations/targets" |
        "/api/v1/operations/recovery" | "/api/v1/platform/force-restart")
        || (path.starts_with("/api/v1/nodes/") && path.ends_with("/agent/force-restart"));
    if !ready.load(Ordering::Acquire) && !recovery {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"message":"数据库尚未就绪，平台处于恢复模式；Agent 管理中的强制重启仍可使用"}))).into_response();
    }
    next.run(request).await
}
