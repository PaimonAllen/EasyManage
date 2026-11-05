//! One shared Markdown reference, loaded on demand and versioned on explicit save.
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row, postgres::PgRow};

const SLUG: &str = "gpu-comparison";
const INITIAL: &str = include_str!("../content/gpu-comparison.md");
const MAX_BYTES: usize = 1024 * 1024;

#[derive(Serialize)]
struct Document {
    content: String,
    revision: i64,
    updated_at: Option<DateTime<Utc>>,
    max_bytes: usize,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum SaveSource {
    #[default]
    Edit,
    Upload,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveRequest {
    content: String,
    expected_revision: i64,
    #[serde(default)]
    source: SaveSource,
}

pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/api/v1/documents/gpu-comparison", get(read).put(save))
        .route("/api/v1/documents/gpu-comparison/download", get(download))
        // JSON escaping can double a valid Markdown file's byte length.
        .layer(DefaultBodyLimit::max(MAX_BYTES * 2 + 4096))
        .with_state(pool)
}

fn fail(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "message": message }))).into_response()
}

fn database_error(error: sqlx::Error) -> Response {
    // No document content in logs or error responses.
    tracing::warn!(code = ?error.as_database_error().and_then(|error| error.code()), "document storage unavailable");
    fail(
        StatusCode::SERVICE_UNAVAILABLE,
        "文档存储暂不可用，请保留草稿后重试",
    )
}

fn from_row(row: PgRow) -> Result<Document, Response> {
    Ok(Document {
        content: row.try_get("content").map_err(database_error)?,
        revision: row.try_get("revision").map_err(database_error)?,
        updated_at: Some(row.try_get("updated_at").map_err(database_error)?),
        max_bytes: MAX_BYTES,
    })
}

async fn current(pool: &PgPool) -> Result<Document, Response> {
    let row =
        sqlx::query("SELECT content,revision,updated_at FROM platform_documents WHERE slug=$1")
            .bind(SLUG)
            .fetch_optional(pool)
            .await
            .map_err(database_error)?;
    row.map(from_row).unwrap_or_else(|| {
        Ok(Document {
            content: INITIAL.to_owned(),
            revision: 0,
            updated_at: None,
            max_bytes: MAX_BYTES,
        })
    })
}

fn uncached(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

async fn read(State(pool): State<PgPool>) -> Result<Response, Response> {
    Ok(uncached(Json(current(&pool).await?).into_response()))
}

fn valid_content(content: &str) -> bool {
    content.len() <= MAX_BYTES
        && !content
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
}

async fn save(
    State(pool): State<PgPool>,
    headers: HeaderMap,
    uri: Uri,
    request: Result<Json<SaveRequest>, JsonRejection>,
) -> Result<Response, Response> {
    crate::operations::validate_origin(&headers, &uri).map_err(IntoResponse::into_response)?;
    let Json(request) =
        request.map_err(|error| fail(error.status(), "文档请求格式不正确或超过大小限制"))?;
    if !valid_content(&request.content)
        || !(0..9_007_199_254_740_991).contains(&request.expected_revision)
    {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "Markdown 必须是有效文本，最大 1 MiB；文档版本无效时请重新读取",
        ));
    }
    let mut tx = pool.begin().await.map_err(database_error)?;
    sqlx::query("SET LOCAL lock_timeout = '2s'")
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    // First save and concurrent initial uploads race on the primary key. Later
    // edits compare and increment the exact revision in one atomic UPDATE.
    let row = if request.expected_revision == 0 {
        sqlx::query("INSERT INTO platform_documents(slug,content,revision) VALUES($1,$2,1) ON CONFLICT(slug) DO NOTHING RETURNING content,revision,updated_at")
            .bind(SLUG).bind(&request.content).fetch_optional(&mut *tx).await
    } else {
        sqlx::query("UPDATE platform_documents SET content=$2,revision=revision+1,updated_at=NOW() WHERE slug=$1 AND revision=$3 RETURNING content,revision,updated_at")
            .bind(SLUG).bind(&request.content).bind(request.expected_revision).fetch_optional(&mut *tx).await
    }.map_err(database_error)?;
    let Some(row) = row else {
        return Err(fail(
            StatusCode::CONFLICT,
            "文档已被其他人更新，当前草稿已保留。请下载草稿，重新读取最新版本后再保存。",
        ));
    };
    let document = from_row(row)?;
    sqlx::query("INSERT INTO audit_logs(actor_type,action,target_type,target_id,details) VALUES('internal-operator','document.save','document',$1,$2)")
        .bind(SLUG).bind(json!({"source":request.source,"revision":document.revision,"bytes":request.content.len(),"sha256":hex::encode(Sha256::digest(request.content.as_bytes()))}))
        .execute(&mut *tx).await.map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(uncached(Json(document).into_response()))
}

async fn download(State(pool): State<PgPool>) -> Result<Response, Response> {
    let document = current(&pool).await?;
    let mut response = Response::new(Body::from(document.content));
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("text/markdown; charset=utf-8"),
    );
    response.headers_mut().insert("content-disposition", HeaderValue::from_static("attachment; filename=\"gpu-comparison.md\"; filename*=UTF-8''GPU_%E6%80%A7%E8%83%BD%E5%AF%B9%E6%AF%94_8%E6%AC%BE.md"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok(uncached(response))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markdown_limits_use_utf8_bytes_and_reject_binary_controls() {
        assert!(valid_content(INITIAL));
        assert!(valid_content("# 中文\r\n\t| GPU |"));
        assert!(valid_content(&"a".repeat(MAX_BYTES)));
        assert!(!valid_content(&"中".repeat(MAX_BYTES / 3 + 1)));
        assert!(!valid_content("text\0binary"));
        assert!(!valid_content("\u{1b}[31m"));
        assert!(
            serde_json::from_str::<SaveRequest>(
                r##"{"content":"# ok","expected_revision":0,"path":"/tmp/escape"}"##
            )
            .is_err()
        );
    }
}
