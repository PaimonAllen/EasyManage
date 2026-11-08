//! Read-only, on-demand history from existing compact rounds. Never runs a probe.
use super::*;
use axum::extract::Query;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OverviewQuery {
    pub(super) node: Option<Uuid>,
    pub(super) days: Option<u8>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LogsQuery {
    pub(super) node: Uuid,
    pub(super) days: Option<u8>,
    pub(super) before: Option<DateTime<Utc>>,
    pub(super) limit: Option<i64>,
}
pub(super) fn days(value: Option<u8>) -> Result<usize, Response> {
    match value.unwrap_or(7) {
        1 => Ok(1),
        7 => Ok(7),
        _ => Err(error(StatusCode::BAD_REQUEST, "历史范围只支持 1 天或 7 天")),
    }
}
pub(super) fn start(now: DateTime<Utc>, days: usize) -> DateTime<Utc> {
    DateTime::from_timestamp(
        (now.timestamp() / history::PERIOD - (days * history::SLOTS) as i64 + 1) * history::PERIOD,
        0,
    )
    .unwrap()
}
async fn profile(state: &Lab, id: Uuid) -> Result<Value, Response> {
    sqlx::query("SELECT id,name,enabled FROM proxy_lab_profiles WHERE id=$1").bind(id)
        .fetch_optional(&state.shared.pool).await.map_err(db)?
        .map(|r|json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"enabled":r.get::<bool,_>("enabled")}))
        .ok_or_else(||error(StatusCode::NOT_FOUND,"配置不存在"))
}
async fn node(state: &Lab, id: Uuid, profile: Uuid) -> Result<Value, Response> {
    sqlx::query("SELECT id,name,protocol,active FROM proxy_lab_nodes WHERE id=$1 AND profile_id=$2")
        .bind(id).bind(profile).fetch_optional(&state.shared.pool).await.map_err(db)?
        .map(|r|json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"protocol":r.get::<String,_>("protocol"),"active":r.get::<bool,_>("active")}))
        .ok_or_else(||error(StatusCode::NOT_FOUND,"节点不属于此配置或已不存在"))
}
pub(super) fn series(
    value: (
        String,
        Option<history::Observation>,
        Option<DateTime<Utc>>,
        usize,
    ),
    count: usize,
) -> Value {
    let (h, latest, at, legacy) = value;
    let up = h.bytes().filter(|v| matches!(v, b'u' | b's')).count();
    let observed = h
        .bytes()
        .filter(|v| matches!(v, b'u' | b's' | b'd'))
        .count();
    json!({"history":h,"latest":latest,"last_check":at,"legacy_samples":legacy,"available_samples":up,"observed_samples":observed,"expected_samples":count,
        "availability_percent":if observed>0{Some(up as f64*100.0/observed as f64)}else{None},"coverage_percent":observed as f64*100.0/count as f64})
}
pub(super) async fn overview(
    State(state): State<Lab>,
    Path(id): Path<Uuid>,
    Query(query): Query<OverviewQuery>,
) -> ApiResult<Value> {
    let days = days(query.days)?;
    let _permit = state
        .shared
        .history_reads
        .try_acquire()
        .map_err(|_| error(StatusCode::TOO_MANY_REQUESTS, "历史查询繁忙，请稍后重试"))?;
    let profile = profile(&state, id).await?;
    let rows=sqlx::query("SELECT id,name,protocol,active FROM proxy_lab_nodes WHERE profile_id=$1 AND active ORDER BY name,id")
        .bind(id).fetch_all(&state.shared.pool).await.map_err(db)?;
    let nodes:Vec<Value>=rows.into_iter().map(|r|json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"protocol":r.get::<String,_>("protocol"),"active":r.get::<bool,_>("active")})).collect();
    let now = Utc::now();
    let start = start(now, days);
    let count = days * history::SLOTS;
    let selected = query.node.or_else(|| {
        nodes
            .first()
            .and_then(|n| n["id"].as_str())
            .and_then(|n| Uuid::parse_str(n).ok())
    });
    let mut output = json!({"profile":profile,"nodes":nodes,"generated_at":now,"window_start":start,"days":days,"slot_seconds":180,"retention_days":7,"selected_node":null,"https":null,"node_link":null});
    if let Some(selected) = selected {
        output["selected_node"] = node(&state, selected, id).await?;
        // Extract one node in PostgreSQL. Do not ship/decode a week's full-fleet
        // JSON, and do not decrypt or select proxy definitions for a history read.
        let rows=sqlx::query("SELECT slot,observation FROM (SELECT slot,jsonb_path_query_first(results,'$[*] ? (@.id == $node)',jsonb_build_object('node',$3::text)) AS observation FROM proxy_lab_rounds WHERE profile_id=$1 AND slot>=$2 AND slot<=NOW() ORDER BY slot LIMIT $4) r WHERE observation IS NOT NULL ORDER BY slot")
            .bind(id).bind(start).bind(selected.to_string()).bind(count as i64).fetch_all(&state.shared.pool).await.map_err(db)?;
        let rounds: Vec<_> = rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<DateTime<Utc>, _>("slot").timestamp(),
                    json!([r.get::<Value, _>("observation")]),
                )
            })
            .collect();
        output["https"] = series(
            history::histories_with_slots(&[selected], start.timestamp(), &rounds, false, count)
                .remove(&selected)
                .unwrap(),
            count,
        );
        output["node_link"] = series(
            history::histories_with_slots(&[selected], start.timestamp(), &rounds, true, count)
                .remove(&selected)
                .unwrap(),
            count,
        );
    }
    Ok(Json(output))
}
pub(super) async fn logs(
    State(state): State<Lab>,
    Path(id): Path<Uuid>,
    Query(query): Query<LogsQuery>,
) -> ApiResult<Value> {
    let days = days(query.days)?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(error(StatusCode::BAD_REQUEST, "每页日志数量应为 1–100"));
    }
    let _permit = state
        .shared
        .history_reads
        .try_acquire()
        .map_err(|_| error(StatusCode::TOO_MANY_REQUESTS, "历史查询繁忙，请稍后重试"))?;
    let profile = profile(&state, id).await?;
    let node = node(&state, query.node, id).await?;
    let now = Utc::now();
    let start = start(now, days);
    let rows=sqlx::query("SELECT slot,checked_at,revision,observation FROM (SELECT slot,checked_at,revision,jsonb_path_query_first(results,'$[*] ? (@.id == $node)',jsonb_build_object('node',$3::text)) AS observation FROM proxy_lab_rounds WHERE profile_id=$1 AND slot>=$2 AND slot<=NOW() AND ($4::TIMESTAMPTZ IS NULL OR slot<$4)) r WHERE observation IS NOT NULL ORDER BY slot DESC LIMIT $5")
        .bind(id).bind(start).bind(query.node.to_string()).bind(query.before).bind(limit+1).fetch_all(&state.shared.pool).await.map_err(db)?;
    let more = rows.len() > limit as usize;
    let entries:Vec<_>=rows.into_iter().take(limit as usize).filter_map(|r|{
        // Deserialize the known observation schema so storage metadata cannot leak
        // arbitrary JSON fields into logs. This never returns credentials or YAML.
        let observation=serde_json::from_value::<history::Observation>(r.get("observation")).ok()?;
        Some(json!({"slot":r.get::<DateTime<Utc>,_>("slot"),"checked_at":r.get::<DateTime<Utc>,_>("checked_at"),"revision":r.get::<i64,_>("revision"),"observation":observation}))
    }).collect();
    let cursor = if more {
        entries.last().map(|e| e["slot"].clone())
    } else {
        None
    };
    Ok(Json(
        json!({"profile":profile,"node":node,"generated_at":now,"window_start":start,"days":days,"retention_days":7,"entries":entries,"next_before":cursor,"has_more":more}),
    ))
}
