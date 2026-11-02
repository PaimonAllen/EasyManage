//! Cooperative pause: stop metrics, retain one small authenticated control poll/min.
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use easymanage_protocol::AgentControl;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

fn fail(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"message":message}))).into_response()
}
fn db(_: sqlx::Error) -> Response {
    fail(StatusCode::SERVICE_UNAVAILABLE, "机器监控设置暂不可用")
}
pub fn from_row(row: &sqlx::postgres::PgRow) -> Result<AgentControl, sqlx::Error> {
    Ok(AgentControl {
        paused: row.try_get("monitoring_paused")?,
        revision: row.try_get::<i64, _>("monitoring_revision")? as u64,
    })
}
pub async fn desired(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    applied: Option<&AgentControl>,
) -> Result<AgentControl, sqlx::Error> {
    let row=sqlx::query("SELECT monitoring_paused,monitoring_revision FROM nodes WHERE id=$1 AND deleted_at IS NULL FOR UPDATE")
        .bind(id).fetch_one(&mut **tx).await?;
    let control = from_row(&row)?;
    if applied == Some(&control) {
        sqlx::query(
            "UPDATE nodes SET monitoring_applied_revision=$2,control_seen_at=NOW() WHERE id=$1 AND (monitoring_applied_revision IS DISTINCT FROM $2 OR control_seen_at IS NULL OR control_seen_at<NOW()-INTERVAL '30 seconds')",
        )
        .bind(id)
        .bind(control.revision as i64)
        .execute(&mut **tx)
        .await?;
    }
    Ok(control)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Change {
    confirmation: Uuid,
    paused: bool,
    expected_revision: u64,
}
pub(crate) async fn change(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    uri: Uri,
    Json(change): Json<Change>,
) -> Result<Json<Value>, Response> {
    crate::operations::validate_origin(&headers, &uri).map_err(IntoResponse::into_response)?;
    if change.confirmation != id {
        return Err(fail(StatusCode::BAD_REQUEST, "请确认原机器 ID"));
    }
    let mut tx = pool.begin().await.map_err(db)?;
    sqlx::query("SET LOCAL lock_timeout = '2s'")
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    let row=sqlx::query("SELECT monitoring_paused,monitoring_revision FROM nodes WHERE id=$1 AND deleted_at IS NULL FOR UPDATE")
        .bind(id).fetch_optional(&mut *tx).await.map_err(db)?.ok_or_else(||fail(StatusCode::NOT_FOUND,"机器不存在或已软删除"))?;
    let before = from_row(&row).map_err(db)?;
    if before.revision != change.expected_revision {
        return Err(fail(
            StatusCode::CONFLICT,
            "机器监控状态已变化，请刷新后重试",
        ));
    }
    let revision = before
        .revision
        .checked_add(1)
        .filter(|v| *v <= i64::MAX as u64)
        .ok_or_else(|| fail(StatusCode::CONFLICT, "监控设置版本已达上限"))?;
    sqlx::query(
        "UPDATE nodes SET monitoring_paused=$2,monitoring_revision=$3,updated_at=NOW() WHERE id=$1",
    )
    .bind(id)
    .bind(change.paused)
    .bind(revision as i64)
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    sqlx::query("INSERT INTO audit_logs(actor_type,actor_id,action,target_type,target_id,details) VALUES('browser','unverified-intranet-operator','machine.monitoring.update','node',$1,$2)")
        .bind(id.to_string()).bind(json!({"before":before,"after":AgentControl{revision,paused:change.paused}})).execute(&mut *tx).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    crate::read_cache::invalidate();
    Ok(Json(
        json!({"control":AgentControl{revision,paused:change.paused},"message":"设置已保存；等待 Agent 确认，不删除身份、历史或更改运行权限。暂停时每分钟只保留轻量控制检查。"}),
    ))
}
