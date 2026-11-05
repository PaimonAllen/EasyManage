//! Shared browser destinations. Never connect to a URL or provision an Agent.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

const MAX_REVISION: i64 = 9_007_199_254_740_991;
const RETURNING: &str = "id,name,url,note,revision,created_at,updated_at,deleted_at";

#[derive(Serialize)]
struct Platform {
    id: Uuid,
    name: String,
    url: String,
    note: String,
    revision: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    name: String,
    url: String,
    #[serde(default)]
    note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    name: String,
    url: String,
    #[serde(default)]
    note: String,
    expected_revision: i64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Delete,
    Restore,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    action: Action,
    confirmation: Uuid,
    expected_revision: i64,
}

pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/api/v1/external-platforms", get(list).post(create))
        .route("/api/v1/external-platforms/{id}", put(update))
        .route("/api/v1/external-platforms/{id}/manage", post(change))
        .layer(DefaultBodyLimit::max(24 * 1024))
        .with_state(pool)
}
fn fail(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"message":message}))).into_response()
}
fn db(error: sqlx::Error) -> Response {
    tracing::warn!(code=?error.as_database_error().and_then(|error|error.code()),"external platform storage unavailable");
    fail(
        StatusCode::SERVICE_UNAVAILABLE,
        "外部平台入口存储暂不可用，请稍后重试",
    )
}
fn row(row: PgRow) -> Result<Platform, Response> {
    Ok(Platform {
        id: row.try_get("id").map_err(db)?,
        name: row.try_get("name").map_err(db)?,
        url: row.try_get("url").map_err(db)?,
        note: row.try_get("note").map_err(db)?,
        revision: row.try_get("revision").map_err(db)?,
        created_at: row.try_get("created_at").map_err(db)?,
        updated_at: row.try_get("updated_at").map_err(db)?,
        deleted_at: row.try_get("deleted_at").map_err(db)?,
    })
}
fn normalize(input: Input) -> Result<Input, Response> {
    let name = input.name.trim();
    let url = input.url.trim();
    let note = input.note.trim();
    if name.is_empty()
        || name.chars().count() > 128
        || name.chars().any(char::is_control)
        || note.chars().count() > 1000
        || note
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\r' | '\n' | '\t'))
    {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "名称必填且最多 128 字；备注最多 1000 字，不能包含无效控制字符",
        ));
    }
    let invalid = || {
        fail(
            StatusCode::BAD_REQUEST,
            "请填写完整的 http:// 或 https:// 链接，不含用户名和密码，最大 4096 字节",
        )
    };
    if url.len() > 4096 || url.chars().any(char::is_control) {
        return Err(invalid());
    }
    let parsed = reqwest::Url::parse(url).map_err(|_| invalid())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.as_str().len() > 4096
    {
        return Err(invalid());
    }
    Ok(Input {
        name: name.to_owned(),
        url: parsed.into(),
        note: note.to_owned(),
    })
}
fn valid_revision(revision: i64) -> Result<(), Response> {
    if !(1..MAX_REVISION).contains(&revision) {
        return Err(fail(StatusCode::BAD_REQUEST, "入口版本无效，请刷新后重试"));
    }
    Ok(())
}
async fn list(State(pool): State<PgPool>) -> Result<Json<Value>, Response> {
    let rows = sqlx::query(&format!(
        "SELECT {RETURNING} FROM external_platforms ORDER BY deleted_at NULLS FIRST,name,id"
    ))
    .fetch_all(&pool)
    .await
    .map_err(db)?;
    let platforms = rows.into_iter().map(row).collect::<Result<Vec<_>, _>>()?;
    Ok(Json(json!({"platforms":platforms})))
}
async fn write(
    pool: &PgPool,
    id: Uuid,
    input: Input,
    revision: Option<i64>,
) -> Result<Json<Platform>, Response> {
    let mut tx = pool.begin().await.map_err(db)?;
    sqlx::query("SET LOCAL lock_timeout='2s'")
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    let result=if let Some(revision)=revision {
        sqlx::query(&format!("UPDATE external_platforms SET name=$2,url=$3,note=$4,revision=revision+1,updated_at=NOW() WHERE id=$1 AND revision=$5 AND deleted_at IS NULL RETURNING {RETURNING}"))
            .bind(id).bind(&input.name).bind(&input.url).bind(&input.note).bind(revision).fetch_optional(&mut *tx).await
    } else {
        sqlx::query(&format!("INSERT INTO external_platforms(id,name,url,note) VALUES($1,$2,$3,$4) RETURNING {RETURNING}"))
            .bind(id).bind(&input.name).bind(&input.url).bind(&input.note).fetch_optional(&mut *tx).await
    }.map_err(db)?;
    let platform =
        row(result.ok_or_else(|| fail(StatusCode::CONFLICT, "入口已被修改或删除，请刷新后重试"))?)?;
    sqlx::query("INSERT INTO audit_logs(actor_type,action,target_type,target_id,details) VALUES('internal-operator',$1,'external-platform',$2,$3)")
        .bind(if revision.is_some(){"external-platform.edit"}else{"external-platform.add"})
        .bind(id.to_string()).bind(json!({"revision":platform.revision})).execute(&mut *tx).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(Json(platform))
}
async fn create(
    State(pool): State<PgPool>,
    headers: HeaderMap,
    uri: Uri,
    Json(input): Json<Input>,
) -> Result<Json<Platform>, Response> {
    crate::operations::validate_origin(&headers, &uri).map_err(IntoResponse::into_response)?;
    write(&pool, Uuid::new_v4(), normalize(input)?, None).await
}
async fn update(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    uri: Uri,
    Json(input): Json<Update>,
) -> Result<Json<Platform>, Response> {
    crate::operations::validate_origin(&headers, &uri).map_err(IntoResponse::into_response)?;
    valid_revision(input.expected_revision)?;
    let data = normalize(Input {
        name: input.name,
        url: input.url,
        note: input.note,
    })?;
    write(&pool, id, data, Some(input.expected_revision)).await
}
async fn change(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    uri: Uri,
    Json(change): Json<Change>,
) -> Result<Json<Platform>, Response> {
    crate::operations::validate_origin(&headers, &uri).map_err(IntoResponse::into_response)?;
    if change.confirmation != id {
        return Err(fail(StatusCode::BAD_REQUEST, "请确认要修改的外部入口"));
    }
    valid_revision(change.expected_revision)?;
    let deleted = matches!(change.action, Action::Delete);
    let mut tx = pool.begin().await.map_err(db)?;
    sqlx::query("SET LOCAL lock_timeout='2s'")
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    let changed=sqlx::query(&format!("UPDATE external_platforms SET deleted_at=CASE WHEN $2 THEN NOW() ELSE NULL END,revision=revision+1,updated_at=NOW() WHERE id=$1 AND revision=$3 AND (deleted_at IS NOT NULL) <> $2 RETURNING {RETURNING}"))
        .bind(id).bind(deleted).bind(change.expected_revision).fetch_optional(&mut *tx).await.map_err(db)?;
    let platform =
        row(changed.ok_or_else(|| fail(StatusCode::CONFLICT, "入口状态已变化，请刷新后重试"))?)?;
    sqlx::query("INSERT INTO audit_logs(actor_type,action,target_type,target_id,details) VALUES('internal-operator',$1,'external-platform',$2,$3)")
        .bind(if deleted{"external-platform.delete"}else{"external-platform.restore"}).bind(id.to_string())
        .bind(json!({"revision":platform.revision})).execute(&mut *tx).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(Json(platform))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_explicit_browser_http_destinations_are_accepted_without_resolving_hosts() {
        let input = |url: &str| Input {
            name: "平台".into(),
            url: url.into(),
            note: String::new(),
        };
        for url in [
            "https://compute.example/path?region=cn#login",
            "http://192.0.2.10:8080/",
            "https://[2001:db8::1]/",
        ] {
            assert!(normalize(input(url)).is_ok());
        }
        for url in [
            "javascript:alert(1)",
            "data:text/html,x",
            "file:///tmp/x",
            "//compute.example",
            "https://user:secret@compute.example",
            "https://good\n.example",
            "",
        ] {
            assert!(normalize(input(url)).is_err());
        }
        assert!(
            normalize(Input {
                name: " ".into(),
                ..input("https://compute.example")
            })
            .is_err()
        );
    }
}
