//! Controller-only YAML analysis, availability and explicitly requested IP reputation.
mod archive;
mod config;
mod dns;
mod engine;
mod fallback;
mod history;
mod local;
mod monitoring;
mod score;
mod vault;
mod worker;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Mutex, Notify, watch};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Clone)]
pub(super) struct Lab {
    shared: Arc<Shared>,
}
struct Shared {
    pool: PgPool,
    vault: vault::Vault,
    notify: Arc<Notify>,
    generations: std::sync::Mutex<HashMap<Uuid, watch::Sender<u64>>>,
    round: std::sync::Mutex<worker::RoundStatus>,
    in_flight: Arc<std::sync::Mutex<std::collections::HashSet<worker::FlightKey>>>,
    mutations: Mutex<()>,
    score_busy: AtomicBool,
    jobs: Mutex<HashMap<Uuid, worker::Job>>,
    history_reads: tokio::sync::Semaphore,
    local_round: std::sync::Mutex<local::LocalRound>,
    monitoring: watch::Sender<monitoring::Settings>,
}
pub(super) struct RuntimeNode {
    id: Uuid,
    revision: i64,
    definition: Value,
    unsupported: Option<String>,
    site_feedback: Vec<history::SiteFeedback>,
}
type ApiResult<T> = Result<Json<T>, Response>;
fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"message":message}))).into_response()
}
fn db(cause: sqlx::Error) -> Response {
    tracing::warn!(code=?cause.as_database_error().and_then(|e|e.code()),"proxy analysis database operation failed");
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "代理分析存储暂不可用，请稍后重试",
    )
}
fn origin(headers: &HeaderMap, uri: &Uri) -> Result<(), Response> {
    crate::operations::validate_origin(headers, uri).map_err(IntoResponse::into_response)
}
pub fn router(pool: PgPool, initialized: crate::database::Initialized) -> anyhow::Result<Router> {
    let state = Lab {
        shared: Arc::new(Shared {
            pool,
            vault: vault::Vault::new()?,
            notify: Arc::new(Notify::new()),
            generations: std::sync::Mutex::new(HashMap::new()),
            round: std::sync::Mutex::new(worker::RoundStatus::default()),
            in_flight: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            mutations: Mutex::new(()),
            score_busy: AtomicBool::new(false),
            jobs: Mutex::new(HashMap::new()),
            history_reads: tokio::sync::Semaphore::new(2),
            local_round: std::sync::Mutex::new(local::LocalRound::default()),
            monitoring: watch::channel(monitoring::Settings::default()).0,
        }),
    };
    worker::start(state.clone(), initialized.clone());
    local::start(state.clone(), initialized);
    Ok(Router::new()
        .route("/api/v1/proxy-lab", get(list))
        .route("/api/v1/proxy-lab/monitoring", put(monitoring::global))
        .route(
            "/api/v1/proxy-lab/local/monitoring",
            put(monitoring::local_source),
        )
        .route("/api/v1/proxy-lab/local", get(local::detail))
        .route("/api/v1/proxy-lab/local/history", get(local::overview))
        .route("/api/v1/proxy-lab/local/logs", get(local::logs))
        .route("/api/v1/proxy-lab/sources", get(sources))
        .route("/api/v1/proxy-lab/profiles", post(import))
        .route(
            "/api/v1/proxy-lab/profiles/{id}",
            get(detail).put(enable).delete(remove),
        )
        .route(
            "/api/v1/proxy-lab/profiles/{id}/history",
            get(archive::overview),
        )
        .route("/api/v1/proxy-lab/profiles/{id}/logs", get(archive::logs))
        .route("/api/v1/proxy-lab/profiles/{id}/name", put(rename))
        .route("/api/v1/proxy-lab/profiles/{id}/score", post(start_score))
        .route("/api/v1/proxy-lab/jobs/{id}", get(job))
        .route(
            "/api/v1/proxy-lab/nodes/{id}/scores/{provider}",
            put(manual_score),
        )
        .layer(DefaultBodyLimit::max(2 * config::MAX_YAML))
        .with_state(state))
}
async fn sources() -> Json<Value> {
    Json(json!({"sources":config::sources().into_iter().map(|(id,p)|{
        json!({"id":id,"name":p.file_name().unwrap_or_default().to_string_lossy(),"bytes":std::fs::metadata(p).map(|m|m.len()).unwrap_or(0)})
    }).collect::<Vec<_>>(),"scope":"platform_local_only"}))
}
async fn list(State(state): State<Lab>) -> ApiResult<Value> {
    let monitoring = monitoring::read(&state).await.map_err(db)?;
    let rows=sqlx::query("SELECT p.id,p.name,p.source,p.original_filename,p.revision,p.enabled,p.warnings,p.updated_at,COUNT(*) OVER(PARTITION BY p.content_hash) AS duplicate_count,(SELECT COUNT(*) FROM proxy_lab_nodes n WHERE n.profile_id=p.id AND n.active) AS nodes,(SELECT MAX(checked_at) FROM proxy_lab_rounds r WHERE r.profile_id=p.id) AS last_check FROM proxy_lab_profiles p ORDER BY p.created_at,p.id")
        .fetch_all(&state.shared.pool).await.map_err(db)?;
    let profiles:Vec<Value>=rows.into_iter().map(|r|json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"source":r.get::<String,_>("source"),"original_filename":r.get::<Option<String>,_>("original_filename"),"duplicate_count":r.get::<i64,_>("duplicate_count"),"revision":r.get::<i64,_>("revision"),"enabled":r.get::<bool,_>("enabled"),"warnings":r.get::<Value,_>("warnings"),"updated_at":r.get::<DateTime<Utc>,_>("updated_at"),"node_count":r.get::<i64,_>("nodes"),"last_check":r.get::<Option<DateTime<Utc>>,_>("last_check")})).collect();
    let round = state
        .shared
        .round
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let active_profiles: Vec<Uuid> = state
        .shared
        .in_flight
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(id, _, _)| *id)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let checking = !active_profiles.is_empty();
    Ok(Json(
        json!({"monitoring":monitoring,"profiles":profiles,"providers":score::PROVIDERS,"check_seconds":180,"window_hours":24,
        "core_available":engine::binary().is_some(),"checking":checking,"checking_profile_id":active_profiles.first(),"checking_profile_ids":active_profiles,"round":round,"queue_mode":"parallel","score_busy":state.shared.score_busy.load(Ordering::Relaxed),"test_url":engine::TEST_SITES[0].url,"test_sites":engine::TEST_SITES,"availability_policy":history::POLICY,"pass_percent_inclusive":80,"retention_days":history::RETENTION_DAYS,"probe_concurrency":null,"legacy_test_sites":engine::LEGACY_TEST_SITES,"restricted_test_sites":engine::RESTRICTED_TEST_SITES,"restricted_fallback_sites":engine::RESTRICTED_FALLBACK_SITES,"fallback_sites":engine::FALLBACK_SITES,"node_link_method":"tcp","site_timeout_ms":8000,"dns_resolver":dns::stats(),"local_network":{"enabled":monitoring.allows(true),"configured_enabled":monitoring.local_enabled,"operator_enabled":local::enabled(),"site_count":local::SITES.len(),"check_seconds":180}}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Import {
    name: String,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    yaml: Option<String>,
    #[serde(default)]
    source_id: Option<String>,
    #[serde(default)]
    replace_id: Option<Uuid>,
    #[serde(default)]
    expected_revision: Option<i64>,
    #[serde(default = "enabled_by_default")]
    enable: bool,
}
fn enabled_by_default() -> bool {
    true
}
fn valid_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}
async fn import(
    State(state): State<Lab>,
    headers: HeaderMap,
    uri: Uri,
    Json(mut input): Json<Import>,
) -> ApiResult<Value> {
    origin(&headers, &uri)?;
    let _lock = state.shared.mutations.lock().await;
    let name = input.name.trim().to_owned();
    if !valid_name(&name) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "配置名称必填，最多 128 字节",
        ));
    }
    if input.filename.as_ref().is_some_and(|v| {
        v.is_empty()
            || v.len() > 255
            || v.chars().any(|c| c.is_control() || matches!(c, '/' | '\\'))
            || matches!(v.as_str(), "." | "..")
    }) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "上传文件名无效；只接受不含路径的文件名",
        ));
    }
    let (text, source, filename) = match (input.yaml.take(), input.source_id) {
        (Some(text), None) => (Zeroizing::new(text), "手工上传".to_owned(), input.filename),
        (None, Some(id)) => {
            let (text, name) =
                config::read_source(&id).map_err(|e| error(StatusCode::BAD_REQUEST, &e))?;
            (Zeroizing::new(text), format!("本机 · {name}"), Some(name))
        }
        _ => {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "请选择一份本机配置或上传一个 YAML 文件",
            ));
        }
    };
    let analysis = config::parse(&text).map_err(|e| error(StatusCode::BAD_REQUEST, &e))?;
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM proxy_lab_profiles")
        .fetch_one(&state.shared.pool)
        .await
        .map_err(db)?;
    state.shared.vault.initialize(existing == 0).map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "平台代理配置主密钥不可用；请保留原密钥并检查私有目录",
        )
    })?;
    let id = input.replace_id.unwrap_or_else(Uuid::new_v4);
    let mut tx = state.shared.pool.begin().await.map_err(db)?;
    sqlx::query("SELECT pg_advisory_xact_lock(874033023)")
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    // A selected UI profile is never enough to authorize replacement. Require
    // an exact configuration-name match under the same write lock as the update.
    let revision = if input.replace_id.is_some() {
        let old =
            sqlx::query("SELECT revision,name FROM proxy_lab_profiles WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db)?
                .ok_or_else(|| error(StatusCode::NOT_FOUND, "配置不存在"))?;
        if old.get::<String, _>("name") != name {
            return Err(error(
                StatusCode::CONFLICT,
                "只有同名配置才能覆盖；不同名称请作为新配置导入",
            ));
        }
        let previous = old.get::<i64, _>("revision");
        if Some(previous) != input.expected_revision {
            return Err(error(StatusCode::CONFLICT, "配置已变化，请刷新后重新上传"));
        }
        previous + 1
    } else {
        if sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM proxy_lab_profiles WHERE name=$1)",
        )
        .bind(&name)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?
        {
            return Err(error(
                StatusCode::CONFLICT,
                "已存在同名配置，请明确勾选覆盖或修改显示名称后导入",
            ));
        }
        1
    };
    let content_hash = hex::encode(Sha256::digest(text.as_bytes()));
    let duplicate=sqlx::query("SELECT id,name,revision,enabled FROM proxy_lab_profiles WHERE content_hash=$1 AND ($2::UUID IS NULL OR id<>$2) ORDER BY created_at,id LIMIT 1 FOR UPDATE")
        .bind(&content_hash).bind(input.replace_id).fetch_optional(&mut *tx).await.map_err(db)?;
    if let Some(duplicate) = duplicate {
        if input.replace_id.is_some() {
            return Err(error(
                StatusCode::CONFLICT,
                "相同内容已存在于另一份配置；请使用已有配置，原配置与历史已保留",
            ));
        }
        let existing_id: Uuid = duplicate.get("id");
        tx.commit().await.map_err(db)?;
        return Ok(Json(
            json!({"id":existing_id,"name":duplicate.get::<String,_>("name"),"revision":duplicate.get::<i64,_>("revision"),"deduplicated":true,"enabled":duplicate.get::<bool,_>("enabled"),"message":"内容与已有配置完全相同，已复用原配置；名称、内容、检测开关及历史均未修改"}),
        ));
    }
    if existing >= 8 && input.replace_id.is_none() {
        return Err(error(
            StatusCode::CONFLICT,
            "最多保留 8 份配置；请删除不需要的配置，或覆盖同名配置并保留历史",
        ));
    }
    sqlx::query("INSERT INTO proxy_lab_profiles(id,name,source,content_hash,revision,enabled,warnings,original_filename) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(id) DO UPDATE SET source=EXCLUDED.source,content_hash=EXCLUDED.content_hash,revision=EXCLUDED.revision,enabled=EXCLUDED.enabled,warnings=EXCLUDED.warnings,original_filename=COALESCE(proxy_lab_profiles.original_filename,EXCLUDED.original_filename),updated_at=NOW()")
        .bind(id).bind(&name).bind(source).bind(content_hash).bind(revision).bind(input.enable).bind(json!(analysis.warnings)).bind(filename).execute(&mut *tx).await.map_err(db)?;
    sqlx::query("UPDATE proxy_lab_nodes SET active=FALSE WHERE profile_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    let rows = sqlx::query("SELECT id,identity_hash FROM proxy_lab_nodes WHERE profile_id=$1")
        .bind(id)
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
    let previous: HashMap<String, Uuid> = rows
        .into_iter()
        .map(|r| (r.get("identity_hash"), r.get("id")))
        .collect();
    for node in &analysis.nodes {
        let node_id = previous
            .get(&node.identity)
            .copied()
            .unwrap_or_else(Uuid::new_v4);
        let plain = Zeroizing::new(
            serde_json::to_vec(&node.definition)
                .map_err(|_| error(StatusCode::BAD_REQUEST, "节点定义无法编码"))?,
        );
        let ciphertext = state
            .shared
            .vault
            .seal(node_id, revision, &plain)
            .map_err(|_| {
                error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "代理凭据加密失败；未保存明文",
                )
            })?;
        sqlx::query("INSERT INTO proxy_lab_nodes(id,profile_id,identity_hash,name,protocol,region,unsupported,definition,revision) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(profile_id,identity_hash) DO UPDATE SET name=EXCLUDED.name,protocol=EXCLUDED.protocol,region=EXCLUDED.region,unsupported=EXCLUDED.unsupported,definition=EXCLUDED.definition,revision=EXCLUDED.revision,active=TRUE")
            .bind(node_id).bind(id).bind(&node.identity).bind(&node.name).bind(&node.protocol).bind(&node.region).bind(&node.unsupported).bind(ciphertext).bind(revision).execute(&mut *tx).await.map_err(db)?;
    }
    tx.commit().await.map_err(db)?;
    state.changed(id);
    Ok(Json(
        json!({"id":id,"name":name,"revision":revision,"node_count":analysis.nodes.len(),"warnings":analysis.warnings,"enabled":input.enable,"message":if input.replace_id.is_some(){"同名配置已覆盖；节点身份和历史保留"}else{"新配置已保存；启用后按总开关状态加入后台检测"}}),
    ))
}
impl Lab {
    fn profile_generation(&self, id: Uuid) -> watch::Receiver<u64> {
        self.shared
            .generations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id)
            .or_insert_with(|| watch::channel(0).0)
            .subscribe()
    }
    fn changed(&self, id: Uuid) {
        self.shared
            .generations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id)
            .or_insert_with(|| watch::channel(0).0)
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        self.shared.notify.notify_one();
    }
    async fn load_nodes(
        &self,
        id: Uuid,
        selected: Option<&[Uuid]>,
    ) -> anyhow::Result<Vec<RuntimeNode>> {
        let rows=sqlx::query("SELECT id,revision,definition,unsupported FROM proxy_lab_nodes WHERE profile_id=$1 AND active AND ($2::UUID[] IS NULL OR id=ANY($2)) ORDER BY id").bind(id).bind(selected).fetch_all(&self.shared.pool).await?;
        let previous=sqlx::query_scalar::<_,Value>("SELECT results FROM proxy_lab_rounds WHERE profile_id=$1 AND revision=(SELECT revision FROM proxy_lab_profiles WHERE id=$1) ORDER BY slot DESC LIMIT 1").bind(id).fetch_optional(&self.shared.pool).await?;
        let mut feedback: HashMap<Uuid, Vec<history::SiteFeedback>> = previous
            .and_then(|v| serde_json::from_value::<Vec<history::Observation>>(v).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|o| o.policy == history::POLICY || o.policy == 4) // Versions 4/5 share the same site catalog and cooldown origins.
            .map(|o| (o.id, o.site_feedback))
            .collect();
        rows.into_iter()
            .map(|r| {
                let id = r.get("id");
                let revision = r.get("revision");
                let ciphertext: Vec<u8> = r.get("definition");
                let plain = self.shared.vault.open(id, revision, &ciphertext)?;
                Ok(RuntimeNode {
                    id,
                    revision,
                    definition: serde_json::from_slice(&plain)
                        .map_err(|_| anyhow::anyhow!("代理节点定义无法读取"))?,
                    unsupported: r.get("unsupported"),
                    site_feedback: feedback.remove(&id).unwrap_or_default(),
                })
            })
            .collect()
    }
    async fn save_score(
        &self,
        id: Uuid,
        revision: i64,
        s: &score::Score,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO proxy_lab_scores(node_id,provider,revision,ip,region,raw_score,purity,direction,state,detail,method,checked_at,observed_at) SELECT $1,$2,$3,$4::TEXT::INET,$5,$6,$7,$8,$9,$10,$11,$12,$13 WHERE EXISTS(SELECT 1 FROM proxy_lab_nodes WHERE id=$1 AND active AND revision=$3) ON CONFLICT(node_id,provider) DO UPDATE SET revision=EXCLUDED.revision,ip=EXCLUDED.ip,region=EXCLUDED.region,raw_score=EXCLUDED.raw_score,purity=EXCLUDED.purity,direction=EXCLUDED.direction,state=EXCLUDED.state,detail=EXCLUDED.detail,method=EXCLUDED.method,checked_at=EXCLUDED.checked_at,observed_at=EXCLUDED.observed_at")
            .bind(id).bind(&s.provider).bind(revision).bind(&s.ip).bind(&s.region).bind(s.raw_score).bind(s.purity).bind(&s.direction).bind(&s.state).bind(&s.detail).bind(&s.method).bind(s.checked_at).bind(s.observed_at).execute(&self.shared.pool).await?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Toggle {
    enabled: bool,
    expected_revision: i64,
    #[serde(default)]
    expected_enabled: Option<bool>,
}
async fn enable(
    State(state): State<Lab>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    uri: Uri,
    Json(input): Json<Toggle>,
) -> ApiResult<Value> {
    origin(&headers, &uri)?;
    let _lock = state.shared.mutations.lock().await;
    let mut tx = state.shared.pool.begin().await.map_err(db)?;
    sqlx::query("SELECT pg_advisory_xact_lock(874033023)")
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    let row = sqlx::query("SELECT revision,enabled FROM proxy_lab_profiles WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "配置不存在"))?;
    if row.get::<i64, _>("revision") != input.expected_revision
        || input
            .expected_enabled
            .is_some_and(|old| old != row.get::<bool, _>("enabled"))
    {
        return Err(error(
            StatusCode::CONFLICT,
            "配置或检测开关已变化，请刷新后重试",
        ));
    }
    sqlx::query("UPDATE proxy_lab_profiles SET enabled=$2,updated_at=NOW() WHERE id=$1")
        .bind(id)
        .bind(input.enabled)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    tx.commit().await.map_err(db)?;
    state.changed(id);
    Ok(Json(
        json!({"enabled":input.enabled,"message":if input.enabled{"本配置已加入后台并行检测；其他配置继续检测"}else{"仅暂停本配置检测，其他配置继续；所有历史保留"}}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rename {
    name: String,
    expected_name: String,
    expected_revision: i64,
}
async fn rename(
    State(state): State<Lab>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    uri: Uri,
    Json(input): Json<Rename>,
) -> ApiResult<Value> {
    origin(&headers, &uri)?;
    let name = input.name.trim();
    if !valid_name(name) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "配置名称必填，最多 128 字节",
        ));
    }
    let changed=sqlx::query("UPDATE proxy_lab_profiles SET name=$2,updated_at=NOW() WHERE id=$1 AND name=$3 AND revision=$4")
        .bind(id).bind(name).bind(&input.expected_name).bind(input.expected_revision).execute(&state.shared.pool).await.map_err(db)?;
    if changed.rows_affected() == 0 {
        return Err(error(
            StatusCode::CONFLICT,
            "配置名称或内容已变化，请刷新后重新命名",
        ));
    }
    Ok(Json(
        json!({"name":name,"message":"显示名已更新；配置内容、节点身份和历史均保留"}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Remove {
    expected_name: String,
    expected_revision: i64,
    confirmation: String,
}
async fn remove(
    State(state): State<Lab>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    uri: Uri,
    Json(input): Json<Remove>,
) -> ApiResult<Value> {
    origin(&headers, &uri)?;
    let _lock = state.shared.mutations.lock().await;
    let mut tx = state.shared.pool.begin().await.map_err(db)?;
    sqlx::query("SET LOCAL lock_timeout='2s'")
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    sqlx::query("SET LOCAL statement_timeout='10s'")
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    sqlx::query("SELECT pg_advisory_xact_lock(874033023)")
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    let profile =
        sqlx::query("SELECT name,revision FROM proxy_lab_profiles WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?
            .ok_or_else(|| error(StatusCode::NOT_FOUND, "配置已不存在，请刷新列表"))?;
    let name: String = profile.get("name");
    if profile.get::<i64, _>("revision") != input.expected_revision || name != input.expected_name {
        return Err(error(
            StatusCode::CONFLICT,
            "配置名称或内容已变化，请刷新后重新确认删除",
        ));
    }
    if input.confirmation != name {
        return Err(error(StatusCode::BAD_REQUEST, "请输入完整配置名称确认删除"));
    }
    // Foreign keys remove only this profile's nodes, ciphertext, history and scores.
    sqlx::query("DELETE FROM proxy_lab_profiles WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    tx.commit().await.map_err(db)?;
    state.changed(id);
    Ok(Json(
        json!({"id":id,"message":"配置及其节点、检测历史和 IP 评分已删除；原 YAML 文件与实际代理服务不受影响"}),
    ))
}
async fn detail(State(state): State<Lab>, Path(id): Path<Uuid>) -> ApiResult<Value> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM proxy_lab_profiles WHERE id=$1)")
            .bind(id)
            .fetch_one(&state.shared.pool)
            .await
            .map_err(db)?;
    if !exists {
        return Err(error(StatusCode::NOT_FOUND, "配置不存在"));
    }
    let now = Utc::now();
    let start = history::window(now);
    let start_date = DateTime::from_timestamp(start, 0).unwrap();
    let rows=sqlx::query("SELECT id,name,protocol,region,unsupported,revision FROM proxy_lab_nodes WHERE profile_id=$1 AND active ORDER BY name,id").bind(id).fetch_all(&state.shared.pool).await.map_err(db)?;
    let rounds=sqlx::query("SELECT slot,results FROM proxy_lab_rounds WHERE profile_id=$1 AND slot>=$2 AND slot<=NOW() ORDER BY slot LIMIT 480").bind(id).bind(start_date).fetch_all(&state.shared.pool).await.map_err(db)?;
    let rounds: Vec<(i64, Value)> = rounds
        .into_iter()
        .map(|r| {
            (
                r.get::<DateTime<Utc>, _>("slot").timestamp(),
                r.get("results"),
            )
        })
        .collect();
    let ids: Vec<Uuid> = rows.iter().map(|r| r.get("id")).collect();
    let mut histories = history::histories(&ids, start, &rounds, false);
    let mut links = history::histories(&ids, start, &rounds, true);
    let score_rows=sqlx::query("SELECT s.node_id,to_jsonb(s)-'node_id'-'ip'||jsonb_build_object('ip',host(s.ip)) AS score FROM proxy_lab_scores s JOIN proxy_lab_nodes n ON n.id=s.node_id WHERE n.profile_id=$1 AND n.active ORDER BY s.checked_at DESC")
        .bind(id).fetch_all(&state.shared.pool).await.map_err(db)?;
    let mut scores: HashMap<Uuid, Vec<Value>> = HashMap::new();
    for r in score_rows {
        scores
            .entry(r.get("node_id"))
            .or_default()
            .push(r.get("score"));
    }
    let nodes:Vec<Value>=rows.into_iter().map(|r|{
        let node:Uuid=r.get("id");let(h,latest,at,legacy)=histories.remove(&node).unwrap();
        let(lh,ll,la,_)=links.remove(&node).unwrap();
        let lu=lh.bytes().filter(|c|matches!(*c,b'u'|b's')).count();let lo=lh.bytes().filter(|c|matches!(*c,b'u'|b's'|b'd')).count();
        let up=h.bytes().filter(|c|*c==b'u'||*c==b's').count();let observed=h.bytes().filter(|c|matches!(*c,b'u'|b's'|b'd')).count();
        json!({"id":node,"name":r.get::<String,_>("name"),"protocol":r.get::<String,_>("protocol"),"region":r.get::<String,_>("region"),"unsupported":r.get::<Option<String>,_>("unsupported"),"revision":r.get::<i64,_>("revision"),
            "node_link":{"history":lh,"latest":ll,"last_check":la,"available_samples":lu,"observed_samples":lo,"expected_samples":480,"availability_percent":if lo>0{Some(lu as f64/lo as f64*100.0)}else{None},"coverage_percent":lo as f64/480.0*100.0},
            "history":h,"legacy_samples":legacy,"latest":latest,"last_check":at,"available_samples":up,"observed_samples":observed,"expected_samples":480,
            "availability_percent":if observed>0{Some(up as f64/observed as f64*100.0)}else{None},"coverage_percent":observed as f64/480.0*100.0,"scores":scores.remove(&node).unwrap_or_default()})
    }).collect();
    Ok(Json(
        json!({"profile_id":id,"window_start":start_date,"generated_at":now,"slot_seconds":180,"nodes":nodes}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScoreInput {
    node_ids: Vec<Uuid>,
}
async fn start_score(
    State(state): State<Lab>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    uri: Uri,
    Json(input): Json<ScoreInput>,
) -> ApiResult<Value> {
    origin(&headers, &uri)?;
    if !monitoring::read(&state).await.map_err(db)?.enabled {
        return Err(error(
            StatusCode::CONFLICT,
            "所有检测已暂停，请先打开总开关",
        ));
    }
    if input.node_ids.is_empty() || input.node_ids.len() > 16 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "每次手动评分选择 1–16 个节点",
        ));
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM proxy_lab_nodes WHERE profile_id=$1 AND active AND id=ANY($2)",
    )
    .bind(id)
    .bind(&input.node_ids)
    .fetch_one(&state.shared.pool)
    .await
    .map_err(db)?;
    if count as usize != input.node_ids.len() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "所选节点重复、已移除或不属于本配置",
        ));
    }
    if state
        .shared
        .score_busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(error(
            StatusCode::CONFLICT,
            "已有手动评分任务运行，请稍后重试",
        ));
    }
    let job = worker::create_job(&state, input.node_ids.len()).await;
    worker::start_score(state, id, input.node_ids, job);
    Ok(Json(json!({"job_id":job})))
}
async fn job(State(state): State<Lab>, Path(id): Path<Uuid>) -> ApiResult<Value> {
    state
        .shared
        .jobs
        .lock()
        .await
        .get(&id)
        .map(|j| Json(json!(j)))
        .ok_or_else(|| {
            error(
                StatusCode::NOT_FOUND,
                "评分任务不存在或平台已重启；已完成的来源结果仍保留",
            )
        })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manual {
    ip: String,
    raw_score: f64,
    direction: String,
    observed_at: DateTime<Utc>,
    expected_revision: i64,
}
async fn manual_score(
    State(state): State<Lab>,
    Path((id, provider)): Path<(Uuid, String)>,
    headers: HeaderMap,
    uri: Uri,
    Json(input): Json<Manual>,
) -> ApiResult<Value> {
    origin(&headers, &uri)?;
    let ip = input
        .ip
        .parse()
        .ok()
        .filter(|ip| engine::public_ip(*ip))
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "请填写该节点实际查询的公网出口 IP"))?;
    if !score::PROVIDERS.iter().any(|p| p.id == provider)
        || !input.raw_score.is_finite()
        || !(0.0..=100.0).contains(&input.raw_score)
        || !matches!(input.direction.as_str(), "risk" | "purity")
        || input.observed_at > Utc::now() + chrono::Duration::minutes(5)
    {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "来源、分值、方向或查询时间无效",
        ));
    }
    let revision =
        sqlx::query_scalar::<_, i64>("SELECT revision FROM proxy_lab_nodes WHERE id=$1 AND active")
            .bind(id)
            .fetch_optional(&state.shared.pool)
            .await
            .map_err(db)?
            .ok_or_else(|| error(StatusCode::NOT_FOUND, "节点不存在"))?;
    if revision != input.expected_revision {
        return Err(error(StatusCode::CONFLICT, "节点已变化，请重新查询出口 IP"));
    }
    let mut value =
        score::Score::unavailable(&provider, "manual", "人工录入的原站结果，未经平台 API 验证");
    value.ip = Some(ip.to_string());
    value.raw_score = Some(input.raw_score);
    value.purity = Some(if input.direction == "risk" {
        100.0 - input.raw_score
    } else {
        input.raw_score
    });
    value.direction = input.direction;
    value.method = "manual".into();
    value.observed_at = Some(input.observed_at);
    state.save_score(id, revision, &value).await.map_err(db)?;
    Ok(Json(
        json!({"message":"已保存人工评分，来源和查询时间会单独标注"}),
    ))
}
