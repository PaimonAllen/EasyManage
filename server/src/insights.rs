//! Bounded, read-only checks from the control plane to the managed nodes.
use crate::performance::PerformanceMonitor;
use axum::{Json, Router, extract::State, routing::get};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::sync::Arc;
use std::{
    collections::HashSet,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio::{net::TcpStream, task::JoinSet};
use uuid::Uuid;
type ObservationCache = Arc<Mutex<Option<(Instant, Value)>>>;

#[derive(Clone)]
struct InsightState {
    collection: crate::collection_settings::CollectionSettings,
    health_cache: ObservationCache,
    gitea_cache: ObservationCache,
    users_cache: ObservationCache,
    pool: PgPool,
    registry: PathBuf,
    gitea: String,
    gitea_admin_token_file: Option<PathBuf>,
    http: reqwest::Client,
    process: PerformanceMonitor,
    hostname: String,
}

#[derive(Deserialize, Clone)]
struct LinkTarget {
    node_id: Uuid,
    hostname: String,
    host: IpAddr,
    port: u16,
    ssh_persistent_connected: Option<bool>,
}

pub fn router(
    pool: PgPool,
    process: PerformanceMonitor,
    collection: crate::collection_settings::CollectionSettings,
) -> anyhow::Result<Router> {
    let registry = std::env::var_os("EASYMANAGE_OPERATIONS_TARGETS").map_or_else(
        || PathBuf::from("/etc/easymanage/targets.json"),
        PathBuf::from,
    );
    let gitea_url = std::env::var("EASYMANAGE_GITEA_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:3000".to_owned());
    let url = reqwest::Url::parse(&gitea_url)?;
    anyhow::ensure!(
        url.scheme() == "http"
            && url
                .host_str()
                .is_some_and(|host| host == "127.0.0.1" || host == "localhost")
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "EASYMANAGE_GITEA_URL must be a local HTTP endpoint"
    );
    let state = InsightState {
        collection,
        health_cache: Arc::new(Mutex::new(None)),
        gitea_cache: Arc::new(Mutex::new(None)),
        users_cache: Arc::new(Mutex::new(None)),
        pool,
        registry,
        gitea: gitea_url.trim_end_matches('/').to_owned(),
        gitea_admin_token_file: std::env::var_os("EASYMANAGE_GITEA_ADMIN_TOKEN_FILE")
            .map(PathBuf::from),
        http: reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()?,
        process,
        hostname: std::fs::read_to_string("/proc/sys/kernel/hostname")
            .map(|value| value.trim().to_owned())
            .unwrap_or_else(|_| "平台主机".to_owned()),
    };
    Ok(Router::new()
        .route("/api/v1/platform-health", get(platform_health))
        .route("/api/v1/gitea", get(gitea))
        .route("/api/v1/gitea/users", get(gitea_users))
        .with_state(state))
}

fn targets(path: &PathBuf) -> Result<Vec<LinkTarget>, &'static str> {
    crate::operations::health_targets(path).map(|targets| {
        targets
            .into_iter()
            .map(
                |(node_id, hostname, host, port, ssh_persistent_connected)| LinkTarget {
                    node_id,
                    hostname,
                    host,
                    port,
                    ssh_persistent_connected,
                },
            )
            .collect()
    })
}

async fn platform_health(State(state): State<InsightState>) -> Json<Value> {
    cached(
        &state.health_cache,
        state.collection.current().platform_health_seconds,
        platform_health_uncached(state.clone()),
    )
    .await
}
async fn gitea(State(state): State<InsightState>) -> Json<Value> {
    cached(
        &state.gitea_cache,
        state.collection.current().gitea_seconds,
        gitea_uncached(state.clone()),
    )
    .await
}
async fn gitea_users(State(state): State<InsightState>) -> Json<Value> {
    cached(
        &state.users_cache,
        state.collection.current().gitea_seconds,
        gitea_users_uncached(state.clone()),
    )
    .await
}
async fn cached(
    cache: &ObservationCache,
    seconds: u64,
    probe: impl std::future::Future<Output = Json<Value>>,
) -> Json<Value> {
    let mut guard = cache.lock().await;
    if let Some((at, value)) = &*guard
        && at.elapsed() < Duration::from_secs(seconds)
    {
        return Json(value.clone());
    }
    let result = probe.await;
    *guard = Some((Instant::now(), result.0.clone()));
    result
}
async fn platform_health_uncached(state: InsightState) -> Json<Value> {
    let started = Instant::now();
    let database = sqlx::query(
        "SELECT n.id, n.hostname, n.agent_version, n.last_seen, n.monitoring_paused, \
                m.collected_at AS agent_process_collected_at, \
                m.metrics->'platform_process' AS agent_process \
         FROM nodes n LEFT JOIN metrics_latest m ON m.node_id = n.id \
         WHERE n.deleted_at IS NULL ORDER BY n.hostname",
    )
    .fetch_all(&state.pool)
    .await;
    let database_latency_ms = started.elapsed().as_millis();
    let db_ok = database.is_ok();
    let (configured, target_error) = match targets(&state.registry) {
        Ok(values) => (values, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    let mut checks = JoinSet::new();
    for target in configured {
        checks.spawn(async move {
            if let Some(connected) = target.ssh_persistent_connected {
                return (target, connected);
            }
            let mut attempt =
                easymanage_runtime::network::counter(easymanage_protocol::NetworkGroup::TcpProbe)
                    .begin();
            let connected = tokio::time::timeout(
                Duration::from_secs(2),
                TcpStream::connect(SocketAddr::new(target.host, target.port)),
            )
            .await;
            attempt.finish(matches!(connected, Ok(Ok(_))));
            (target, matches!(connected, Ok(Ok(_))))
        });
    }
    let mut links = Vec::new();
    while let Some(Ok((target, ssh_tcp_reachable))) = checks.join_next().await {
        links.push(json!({
            "node_id": target.node_id, "hostname": target.hostname,
            "host": target.host, "port": target.port,
            "ssh_tcp_reachable": ssh_tcp_reachable,
        }));
    }
    links.sort_by(|left, right| left["hostname"].as_str().cmp(&right["hostname"].as_str()));
    let now = Utc::now();
    let nodes: Vec<Value> = database
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| {
            let id: Uuid = row.try_get("id").ok()?;
            let last_seen: DateTime<Utc> = row.try_get("last_seen").ok()?;
            Some(json!({
                "node_id": id,
                "hostname": row.try_get::<String, _>("hostname").ok()?,
                "agent_version": row.try_get::<String, _>("agent_version").ok()?,
                "last_seen": last_seen,
                "monitoring_paused": row.try_get::<bool, _>("monitoring_paused").ok()?,
                "agent_to_platform_healthy": (now - last_seen).num_seconds() <= 45,
                "agent_process": row.try_get::<Option<Value>, _>("agent_process").ok().flatten(),
                "agent_process_collected_at": row.try_get::<Option<DateTime<Utc>>, _>("agent_process_collected_at").ok().flatten(),
            }))
        })
        .collect();
    let platform_process = state.process.current().await;
    Json(json!({
        "generated_at": now,
        "platform": { "api_healthy": true, "database_healthy": db_ok,
            "database_latency_ms": database_latency_ms, "version": env!("CARGO_PKG_VERSION") },
        "platform_process": platform_process.as_ref().map(|sample| &sample.metric),
        "platform_process_collected_at": platform_process.as_ref().map(|sample| sample.sampled_at),
        "platform_hostname": state.hostname,
        "sample_policy": {
            "server_process_seconds": state.collection.current().server_process_seconds,
            "report_seconds": state.collection.current().report_seconds,
        },
        "nodes": nodes,
        "platform_to_nodes": links,
        "target_error": target_error,
        "check_meaning": "SSH TCP reachable checks port access only; Agent heartbeat proves Agent-to-platform reporting, not node-to-node reachability",
    }))
}

async fn gitea_uncached(state: InsightState) -> Json<Value> {
    Json(gitea_observation(&state.http, &state.gitea).await)
}

// Gitea returns HTTP 424 with useful dependency checks when its database is
// down. Keep that evidence, and query the version independently of health.
async fn gitea_observation(http: &reqwest::Client, base: &str) -> Value {
    let started = Instant::now();
    let health = crate::network::service_request(
        http.get(format!("{base}/api/healthz")),
        easymanage_protocol::NetworkGroup::HttpService,
    )
    .await;
    let (healthy, checks, message, health_http_status) = match health {
        Ok(response) => {
            let body = response.json().unwrap_or(Value::Null);
            let checks = gitea_checks(&body);
            let passed = response.status.is_success() && body["status"] == "pass";
            let database_failed = checks["database:ping"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item["status"] == "fail"));
            let message = if passed {
                "健康".to_owned()
            } else if database_failed {
                format!("Gitea 数据库连接失败（HTTP {}）", response.status.as_u16())
            } else if matches!(body["status"].as_str(), Some("fail" | "warn")) {
                format!("健康检查未通过（HTTP {}）", response.status.as_u16())
            } else {
                format!(
                    "健康接口返回错误或无效数据（HTTP {}）",
                    response.status.as_u16()
                )
            };
            (passed, checks, message, Some(response.status.as_u16()))
        }
        Err(_) => (false, Value::Null, "无法连接本机 Gitea".to_owned(), None),
    };
    let version: Option<String> = match crate::network::service_request(
        http.get(format!("{base}/api/v1/version")),
        easymanage_protocol::NetworkGroup::HttpService,
    )
    .await
    {
        Ok(response) if response.status.is_success() => response
            .json()
            .ok()
            .and_then(|body| body["version"].as_str().map(str::to_owned)),
        _ => None,
    };
    json!({ "generated_at": Utc::now(), "healthy": healthy,
        "message": message, "version": version, "latency_ms": started.elapsed().as_millis(),
        "health_http_status": health_http_status,
        "checks": checks, "source": "本机 Gitea /api/healthz 与 /api/v1/version" })
}

fn gitea_checks(body: &Value) -> Value {
    let Some(checks) = body["checks"].as_object() else {
        return Value::Null;
    };
    let mut result = serde_json::Map::new();
    for (name, items) in checks.iter().take(32) {
        if name.len() > 128 {
            continue;
        }
        let Some(items) = items.as_array() else {
            continue;
        };
        let items: Vec<Value> = items
            .iter()
            .take(32)
            .filter_map(|item| {
                let status = item["status"]
                    .as_str()
                    .filter(|value| matches!(*value, "pass" | "fail" | "warn"))?;
                // Do not return arbitrary upstream error messages, SQL or secrets.
                let time = item["time"]
                    .as_str()
                    .filter(|value| chrono::DateTime::parse_from_rfc3339(value).is_ok());
                Some(json!({"status": status, "time": time}))
            })
            .collect();
        result.insert(name.clone(), Value::Array(items));
    }
    Value::Object(result)
}

#[derive(Deserialize)]
struct GiteaUser {
    id: i64,
    login: String,
    full_name: Option<String>,
    active: Option<bool>,
    is_admin: Option<bool>,
    prohibit_login: Option<bool>,
    restricted: Option<bool>,
    created: Option<String>,
    last_login: Option<String>,
}

fn gitea_admin_token(path: Option<&PathBuf>) -> Result<Option<String>, &'static str> {
    let Some(path) = path else { return Ok(None) };
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| "Gitea 管理员只读令牌文件不可读取")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !path.is_absolute() || metadata.permissions().mode() & 0o077 != 0 {
            return Err("Gitea 令牌文件必须使用绝对路径，且权限不宽于 0600");
        }
    }
    if !metadata.is_file() || metadata.len() > 4096 {
        return Err("Gitea 令牌文件格式不安全");
    }
    let value = std::fs::read_to_string(path).map_err(|_| "Gitea 管理员只读令牌文件不可读取")?;
    let token = value.trim();
    if token.len() < 10 || token.len() > 512 || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err("Gitea 令牌文件内容无效");
    }
    Ok(Some(token.to_owned()))
}

async fn gitea_users_uncached(state: InsightState) -> Json<Value> {
    let token = match gitea_admin_token(state.gitea_admin_token_file.as_ref()) {
        Ok(token) => token,
        Err(message) => {
            return Json(json!({ "generated_at": Utc::now(), "scope": "unavailable",
            "users": [], "total_count": null, "truncated": false, "message": message }));
        }
    };
    let verified = token.is_some();
    let mut users = Vec::new();
    let mut seen = HashSet::new();
    let mut total_count: Option<usize> = None;
    let mut complete = false;
    // Bound the read-only scan to 500 accounts; do not let dashboard traffic exhaust Gitea.
    for page in 1..=10 {
        let endpoint = if verified {
            "admin/users"
        } else {
            "users/search"
        };
        let mut request = state
            .http
            .get(format!("{}/api/v1/{endpoint}", state.gitea))
            .query(&[("page", page), ("limit", 50)]);
        if let Some(token) = &token {
            request = request
                .query(&[("sort", "id"), ("order", "asc")])
                .header(reqwest::header::AUTHORIZATION, format!("token {token}"));
        }
        let response = match crate::network::service_request(
            request,
            easymanage_protocol::NetworkGroup::HttpService,
        )
        .await
        {
            Ok(response) if response.status.is_success() => response,
            Ok(response) => {
                let message = if verified
                    && (response.status.as_u16() == 401 || response.status.as_u16() == 403)
                {
                    "Gitea 令牌无权读取管理员用户清单；请检查账户管理员身份及 read:admin 权限"
                } else {
                    "Gitea 用户接口返回错误"
                };
                let message = format!("{message}（HTTP {}）", response.status.as_u16());
                return Json(json!({ "generated_at": Utc::now(), "scope": "unavailable",
                    "users": [], "total_count": null, "truncated": false, "message": message }));
            }
            Err(_) => {
                return Json(json!({ "generated_at": Utc::now(), "scope": "unavailable",
                "users": [], "total_count": null, "truncated": false, "message": "无法连接本机 Gitea 用户接口" }));
            }
        };
        if let Some(count) = response
            .headers
            .get("x-total-count")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
        {
            total_count = Some(count);
        }
        let body = match response.json() {
            Ok(body) => body,
            Err(_) => {
                return Json(json!({ "generated_at": Utc::now(), "scope": "unavailable",
                "users": [], "total_count": null, "truncated": false, "message": "Gitea 用户响应格式无效" }));
            }
        };
        let records = if verified {
            body.as_array()
        } else {
            body["data"].as_array()
        };
        if !verified && body["ok"] != true {
            return Json(json!({ "generated_at": Utc::now(), "scope": "unavailable",
                "users": [], "total_count": null, "truncated": false, "message": "Gitea 公开用户查询未成功" }));
        }
        let Some(records) = records else {
            return Json(json!({ "generated_at": Utc::now(), "scope": "unavailable",
                "users": [], "total_count": null, "truncated": false, "message": "Gitea 用户响应格式无效" }));
        };
        for record in records {
            let Ok(user) = serde_json::from_value::<GiteaUser>(record.clone()) else {
                return Json(json!({ "generated_at": Utc::now(), "scope": "unavailable",
                    "users": [], "total_count": null, "truncated": false, "message": "Gitea 用户记录格式无效" }));
            };
            if user.login.is_empty() {
                return Json(json!({ "generated_at": Utc::now(), "scope": "unavailable",
                    "users": [], "total_count": null, "truncated": false, "message": "Gitea 用户记录缺少用户名" }));
            }
            if !seen.insert(user.id) {
                continue;
            }
            let status = if !verified {
                "unknown"
            } else if user.prohibit_login == Some(true) {
                "prohibited"
            } else if user.active == Some(false) {
                "inactive"
            } else if user.restricted == Some(true) {
                "restricted"
            } else if user.active == Some(true)
                && user.prohibit_login == Some(false)
                && user.restricted == Some(false)
            {
                "normal"
            } else {
                "unknown"
            };
            users.push(json!({ "id": user.id, "login": user.login,
                "full_name": user.full_name, "created": user.created,
                "last_login": if verified { user.last_login } else { None },
                "is_admin": if verified { user.is_admin } else { None }, "account_status": status }));
        }
        if total_count.is_some_and(|count| seen.len() >= count)
            || (total_count.is_none() && records.len() < 50)
        {
            complete = true;
            break;
        }
        if records.is_empty() {
            break;
        }
    }
    let scope = if verified && complete {
        "complete"
    } else if verified {
        "partial"
    } else {
        "public"
    };
    let message = if verified && complete {
        "管理员只读接口已核验账户状态"
    } else if verified {
        "账户超过扫描上限或分页结果不完整，仅显示部分账户"
    } else {
        "仅列出公开可见账户；未认证接口不能可靠判断账户状态，也可能漏掉私有账户"
    };
    Json(json!({ "generated_at": Utc::now(), "scope": scope,
        "users": users, "total_count": total_count, "truncated": !complete,
        "message": message }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn observation(status: &str, body: Value) -> Value {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_owned();
        let fixture = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 8192];
                let length = stream.read(&mut request).await.unwrap();
                let health = request[..length].starts_with(b"GET /api/healthz ");
                assert!(health || request[..length].starts_with(b"GET /api/v1/version "));
                let (status, body) = if health {
                    (status.as_str(), body.to_string())
                } else {
                    ("200 OK", json!({"version":"1.26.2"}).to_string())
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let result = gitea_observation(&client, &format!("http://{address}")).await;
        fixture.await.unwrap();
        result
    }

    #[tokio::test]
    async fn gitea_failed_dependency_keeps_evidence_and_independent_version() {
        let result = observation("424 Failed Dependency", json!({"status":"fail", "checks":{
            "database:ping":[{"status":"fail","time":"2026-11-05T06:56:37Z", "output":"private-upstream-database-error"}],
            "cache:ping":[{"status":"pass","time":"2026-11-05T06:56:37Z"}]
        }})).await;
        assert_eq!(result["healthy"], false);
        assert_eq!(result["version"], "1.26.2");
        assert_eq!(result["health_http_status"], 424);
        assert_eq!(result["checks"]["database:ping"][0]["status"], "fail");
        assert_eq!(result["checks"]["cache:ping"][0]["status"], "pass");
        assert!(
            result["message"]
                .as_str()
                .unwrap()
                .contains("数据库连接失败")
        );
        assert!(!result.to_string().contains("private-upstream"));
    }

    #[tokio::test]
    async fn gitea_recovery_and_invalid_responses_are_distinct() {
        let healthy = observation("200 OK", json!({"status":"pass","checks":{}})).await;
        assert_eq!(healthy["healthy"], true);
        for (status, body) in [
            ("200 OK", json!({"error":"private-error"})),
            ("502 Bad Gateway", json!({"status":"pass"})),
        ] {
            let failed = observation(status, body).await;
            assert_eq!(failed["healthy"], false);
            assert_eq!(failed["version"], "1.26.2");
            assert!(!failed.to_string().contains("private-error"));
        }
    }

    #[test]
    fn gitea_checks_reject_invalid_shapes_and_private_fields() {
        let checks = gitea_checks(&json!({"checks":{
            "invalid":"text", "database:ping":[{"status":"private-text"},
            {"status":"fail","time":"private-time","output":"private-error"}]
        }}));
        assert!(checks.get("invalid").is_none());
        assert_eq!(checks["database:ping"].as_array().unwrap().len(), 1);
        assert_eq!(checks["database:ping"][0]["time"], Value::Null);
        assert!(!checks.to_string().contains("private-"));
    }
}
