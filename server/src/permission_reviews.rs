//! Shared operator annotations, never replacements for automated permission evidence.
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use easymanage_protocol::{PermissionAudit, PermissionCheck};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewRequest {
    check: PermissionCheck,
    accepted: bool,
    #[serde(default)]
    note: String,
}

#[derive(Serialize)]
struct Review {
    node_id: Uuid,
    check: Value,
    note: String,
    marked_at: DateTime<Utc>,
}

pub fn router(pool: PgPool, settings: crate::collection_settings::CollectionSettings) -> Router {
    Router::new()
        .route("/api/v1/permission-reviews", get(list))
        .route("/api/v1/nodes/{node_id}/permission-review", post(mark))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state((pool, settings))
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "message": message }))).into_response()
}

fn database_error(cause: sqlx::Error) -> Response {
    tracing::error!(error = %cause, "permission review storage failed");
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "人工标记存储暂不可用，请稍后重试",
    )
}

async fn list(
    State((pool, _)): State<(PgPool, crate::collection_settings::CollectionSettings)>,
) -> Result<Json<Value>, Response> {
    let rows = sqlx::query("SELECT node_id, evidence, note, marked_at FROM permission_reviews ORDER BY marked_at DESC LIMIT 4097")
        .fetch_all(&pool).await.map_err(database_error)?;
    if rows.len() > 4096 {
        return Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "人工标记数量超过展示上限",
        ));
    }
    let reviews = rows
        .iter()
        .map(|row| {
            Ok(Review {
                node_id: row.try_get("node_id")?,
                check: row.try_get("evidence")?,
                note: row.try_get("note")?,
                marked_at: row.try_get("marked_at")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(database_error)?;
    Ok(Json(json!({ "reviews": reviews })))
}

fn same_check(left: &PermissionCheck, right: &PermissionCheck) -> bool {
    left.category == right.category
        && left.title == right.title
        && left.path == right.path
        && left.status == right.status
        && left.observed == right.observed
        && left.expected == right.expected
        && left.detail == right.detail
}

async fn mark(
    State((pool, settings)): State<(PgPool, crate::collection_settings::CollectionSettings)>,
    Path(node_id): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
    Json(request): Json<ReviewRequest>,
) -> Result<Json<Value>, Response> {
    crate::operations::validate_origin(&headers, &uri).map_err(IntoResponse::into_response)?;
    let check = &request.check;
    if !matches!(
        check.category.as_str(),
        "home" | "shared" | "project" | "sudo" | "root"
    ) || check.title.is_empty()
        || check.title.len() > 256
        || check.path.is_empty()
        || check.path.len() > 1024
        || request.note.chars().count() > 1000
    {
        return Err(error(StatusCode::BAD_REQUEST, "检查项或备注格式不正确"));
    }
    let mut tx = pool.begin().await.map_err(database_error)?;
    // Lock in the same order as report ingestion: node, then latest metrics.
    let node = sqlx::query("SELECT last_seen FROM nodes WHERE id=$1 FOR SHARE")
        .bind(node_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "节点不存在"))?;
    // Serialize with a switch change on the node row, not a pre-lock cache read.
    if request.accepted
        && !sqlx::query_scalar::<_, bool>(
            "SELECT enabled FROM node_permission_collection WHERE node_id=$1",
        )
        .bind(node_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .unwrap_or(true)
    {
        return Err(error(
            StatusCode::CONFLICT,
            "权限检查采集已关闭，不能将旧快照标记为当前人工 OK",
        ));
    }
    let evidence = if request.accepted {
        if !matches!(
            check.status.as_str(),
            "warning" | "critical" | "unknown" | "missing"
        ) {
            return Err(error(StatusCode::BAD_REQUEST, "自动通过项无需人工标记"));
        }
        let last_seen: DateTime<Utc> = node.try_get("last_seen").map_err(database_error)?;
        let latest = sqlx::query("SELECT metrics FROM metrics_latest WHERE node_id=$1 FOR SHARE")
            .bind(node_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(database_error)?
            .ok_or_else(|| error(StatusCode::CONFLICT, "尚无权限检查数据"))?;
        let metrics: Value = latest.try_get("metrics").map_err(database_error)?;
        let audit = metrics
            .get("permission_audit")
            .cloned()
            .and_then(|value| serde_json::from_value::<PermissionAudit>(value).ok())
            .ok_or_else(|| error(StatusCode::CONFLICT, "尚无权限检查数据"))?;
        let now = Utc::now();
        let policy = settings.current();
        let allowed_age = (policy.permission_seconds + policy.report_seconds + 120).max(480) as i64;
        if (now - last_seen).num_seconds() > 45
            || (now - audit.collected_at).num_seconds() > allowed_age
            || audit.collected_at > now + chrono::Duration::seconds(30)
        {
            return Err(error(
                StatusCode::CONFLICT,
                "节点离线或权限快照已过期，请刷新后重新复核",
            ));
        }
        if !audit
            .checks
            .iter()
            .any(|current| same_check(current, check))
        {
            return Err(error(
                StatusCode::CONFLICT,
                "检查内容已变化，请刷新后重新复核",
            ));
        }
        let value = serde_json::to_value(check)
            .map_err(|_| error(StatusCode::BAD_REQUEST, "检查内容无效"))?;
        sqlx::query("INSERT INTO permission_reviews (node_id,category,title,path,evidence,note) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (node_id,category,title,path) DO UPDATE SET evidence=EXCLUDED.evidence,note=EXCLUDED.note,marked_at=NOW()")
            .bind(node_id).bind(&check.category).bind(&check.title).bind(&check.path)
            .bind(&value).bind(request.note.trim()).execute(&mut *tx).await.map_err(database_error)?;
        value
    } else {
        let row = sqlx::query("DELETE FROM permission_reviews WHERE node_id=$1 AND category=$2 AND title=$3 AND path=$4 RETURNING evidence")
            .bind(node_id).bind(&check.category).bind(&check.title).bind(&check.path)
            .fetch_optional(&mut *tx).await.map_err(database_error)?;
        let Some(row) = row else {
            return Ok(Json(json!({ "status": "ok" })));
        };
        row.try_get("evidence").map_err(database_error)?
    };
    sqlx::query("INSERT INTO audit_logs(actor_type,action,target_type,target_id,details) VALUES ('internal-operator',$1,'node',$2,$3)")
        .bind(if request.accepted { "permission.review.accept" } else { "permission.review.remove" })
        .bind(node_id.to_string())
        .bind(json!({ "check": evidence, "note": request.note.trim(), "identity_verified": false }))
        .execute(&mut *tx).await.map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(Json(json!({ "status": "ok" })))
}

/// Invalidate annotations on changed/removed evidence, using the accepted latest
/// snapshot, not a delayed report. Keep the old acceptance in the audit history.
pub async fn invalidate_changed(
    tx: &mut Transaction<'_, Postgres>,
    node_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(r"
        WITH invalidated AS (
            DELETE FROM permission_reviews r WHERE r.node_id=$1 AND NOT EXISTS (
                -- Explicitly paused/warming-up new collectors have no new evidence.
                -- Keep annotations dormant; a fresh audit still invalidates changes.
                SELECT 1 FROM metrics_latest m WHERE m.node_id=r.node_id
                AND m.metrics->'collection_policy'->>'permissions_enabled' IN ('true','false')
                AND (m.metrics->'permission_audit' IS NULL OR m.metrics->'permission_audit'='null'::jsonb)
            ) AND NOT EXISTS (
                SELECT 1 FROM metrics_latest m,
                    jsonb_array_elements(COALESCE(NULLIF(m.metrics->'permission_audit'->'checks','null'::jsonb),'[]'::jsonb)) AS checks(evidence)
                WHERE m.node_id=r.node_id AND checks.evidence=r.evidence
            ) RETURNING node_id,evidence,note,marked_at
        )
        INSERT INTO audit_logs(actor_type,action,target_type,target_id,details)
        SELECT 'system','permission.review.invalidated','node',node_id::text,
            jsonb_build_object('check',evidence,'note',note,'marked_at',marked_at,'reason','evidence_changed_or_removed')
        FROM invalidated
    ").bind(node_id).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn annotation_requires_identical_evidence() {
        let first = PermissionCheck {
            category: "home".into(),
            title: "fixture".into(),
            path: "/home/fixture".into(),
            status: "warning".into(),
            observed: "2770".into(),
            expected: "0700".into(),
            detail: "group access".into(),
        };
        let mut changed = first.clone();
        assert!(same_check(&first, &changed));
        changed.observed = "0777".into();
        assert!(!same_check(&first, &changed));
        changed = first.clone();
        changed.status = "critical".into();
        assert!(!same_check(&first, &changed));
        changed = first.clone();
        changed.expected = "root only".into();
        assert!(!same_check(&first, &changed));
    }
}
