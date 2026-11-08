//! Direct platform-egress HTTPS connectivity. No proxy, Agent, or website bodies.
use super::*;
use axum::extract::Query;
use futures_util::{StreamExt, stream};
use serde::Serialize;
use std::time::Duration;

#[derive(Clone, Copy, Serialize)]
pub(super) struct Site {
    pub name: &'static str,
    pub url: &'static str,
}
pub(super) const SITES: [Site; 20] = [
    Site {
        name: "百度",
        url: "https://www.baidu.com/",
    },
    Site {
        name: "腾讯网",
        url: "https://www.qq.com/",
    },
    Site {
        name: "微信",
        url: "https://weixin.qq.com/",
    },
    Site {
        name: "淘宝",
        url: "https://www.taobao.com/",
    },
    Site {
        name: "天猫",
        url: "https://www.tmall.com/",
    },
    Site {
        name: "京东",
        url: "https://www.jd.com/",
    },
    Site {
        name: "拼多多",
        url: "https://www.pinduoduo.com/",
    },
    Site {
        name: "哔哩哔哩",
        url: "https://www.bilibili.com/",
    },
    Site {
        name: "抖音",
        url: "https://www.douyin.com/",
    },
    Site {
        name: "快手",
        url: "https://www.kuaishou.com/",
    },
    Site {
        name: "微博",
        url: "https://weibo.com/",
    },
    Site {
        name: "知乎",
        url: "https://www.zhihu.com/",
    },
    Site {
        name: "网易",
        url: "https://www.163.com/",
    },
    Site {
        name: "新浪",
        url: "https://www.sina.com.cn/",
    },
    Site {
        name: "搜狐",
        url: "https://www.sohu.com/",
    },
    Site {
        name: "360 搜索",
        url: "https://www.so.com/",
    },
    Site {
        name: "央视网",
        url: "https://www.cctv.com/",
    },
    Site {
        name: "新华网",
        url: "https://www.xinhuanet.com/",
    },
    Site {
        name: "人民网",
        url: "https://www.people.com.cn/",
    },
    Site {
        name: "中国政府网",
        url: "https://www.gov.cn/",
    },
];
pub(super) const SOURCE: &str = "local_network";
fn site_id(site: Site) -> Uuid {
    let mut hash = Sha256::new();
    hash.update(b"easymanage:local-site:v1:");
    hash.update(site.url.as_bytes());
    let mut bytes: [u8; 16] = hash.finalize()[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}
fn sites() -> Vec<Value> {
    SITES.iter().map(|s|json!({"id":site_id(*s),"name":s.name,"url":s.url,"protocol":"https","active":true})).collect()
}
fn find_site(id: Uuid) -> Result<Site, Response> {
    SITES
        .into_iter()
        .find(|s| site_id(*s) == id)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "网站不属于本机网络检测目录"))
}
pub(super) fn enabled() -> bool {
    !std::env::var("EASYMANAGE_LOCAL_NETWORK_ENABLED")
        .is_ok_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "false" | "0"))
}
#[derive(Clone, Default, Serialize)]
pub(super) struct LocalRound {
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    next_run_at: Option<DateTime<Utc>>,
    duration_ms: Option<u64>,
    expected_sites: usize,
    completed_sites: usize,
    saved_sites: usize,
    error: Option<String>,
    paused: bool,
}
fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .no_proxy()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(8))
        .pool_max_idle_per_host(0)
        .user_agent("EasyManage/0.5 (platform network connectivity)")
        .build()
}
fn empty(site: Site) -> history::Observation {
    history::Observation {
        id: site_id(site),
        state: "unknown".into(),
        latency_ms: None,
        http_status: None,
        http_fallback: None,
        detail: String::new(),
        policy: 2,
        sites: vec![],
        link: None,
        site_feedback: vec![],
    }
}
fn failure(error: &reqwest::Error) -> (&'static str, &'static str, bool) {
    use std::error::Error;
    let mut cause: Option<&(dyn Error + 'static)> = Some(error);
    let mut tls = false;
    while let Some(e) = cause {
        if e.downcast_ref::<std::io::Error>()
            .is_some_and(|e| matches!(e.raw_os_error(), Some(12 | 23 | 24 | 105)))
        {
            return ("unknown", "平台本地连接资源不足，无法判断外网连通性", false);
        }
        tls |= e.to_string().to_ascii_lowercase().contains("certificate");
        cause = e.source();
    }
    if tls {
        ("down", "HTTPS 证书验证失败", false)
    } else if error.is_builder() {
        ("unknown", "本机 HTTPS 检测客户端不可用", false)
    } else if error.is_timeout() {
        ("down", "本机到网站的 HTTPS 连接或响应超时", true)
    } else {
        ("down", "本机到网站的 HTTPS 连接失败", true)
    }
}
async fn probe(client: &reqwest::Client, site: Site) -> history::Observation {
    let mut value = empty(site);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(18);
    for index in 0..2 {
        let counters =
            easymanage_runtime::network::counter(easymanage_protocol::NetworkGroup::HttpProbe);
        let mut attempt = counters.begin();
        let at = tokio::time::Instant::now();
        match client.head(site.url).send().await {
            Ok(reply) => {
                // A verified TLS/HTTP reply proves network reachability, including
                // explicit website denials. Preserve its status; never call it an
                // application success or download/follow its body/redirect target.
                let ms = at.elapsed().as_millis().max(1) as u64;
                value.state = if ms > 1500 { "slow" } else { "up" }.into();
                value.latency_ms = Some(ms);
                value.http_status = Some(reply.status().as_u16());
                value.detail.clear();
                attempt.finish(reply.status().is_success());
                // Some public websites route HEAD differently from GET. Only
                // method/path compatibility responses get one safe GET check;
                // authentication denials and rate limits are never retried here.
                let head_status = reply.status().as_u16();
                drop(reply);
                if matches!(head_status, 404 | 405 | 501) {
                    return compatible_get(client, site, value, head_status, deadline).await;
                }
                return value;
            }
            Err(error) => {
                attempt.finish(false);
                let (state, detail, retry) = failure(&error);
                value.state = state.into();
                value.detail = detail.into();
                if index == 0 && retry {
                    tokio::time::sleep(Duration::from_millis(
                        300 + (site_id(site).as_u128() % 400) as u64,
                    ))
                    .await;
                } else {
                    return value;
                }
            }
        }
    }
    value
}
async fn compatible_get(
    client: &reqwest::Client,
    site: Site,
    mut value: history::Observation,
    head_status: u16,
    deadline: tokio::time::Instant,
) -> history::Observation {
    value.http_fallback = Some(history::HttpFallback {
        head_status,
        get_status: None,
    });
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        value.detail = "GET 兼容复核未执行：单站检测预算已用完，保留 HEAD 的 HTTPS 可达证据".into();
        return value;
    }
    let mut attempt =
        easymanage_runtime::network::counter(easymanage_protocol::NetworkGroup::HttpProbe).begin();
    let at = tokio::time::Instant::now();
    // send() resolves on response headers. Drop the response immediately: never
    // consume, decode or save the body, execute scripts, or follow redirects.
    match client
        .get(site.url)
        .timeout(remaining.min(Duration::from_secs(8)))
        .header("Accept-Encoding", "identity")
        .send()
        .await
    {
        Ok(reply) => {
            let status = reply.status().as_u16();
            let ms = at.elapsed().as_millis().max(1) as u64;
            value.state = if ms > 1500 { "slow" } else { "up" }.into();
            value.latency_ms = Some(ms);
            value.http_status = Some(status);
            value.http_fallback.as_mut().unwrap().get_status = Some(status);
            attempt.finish(reply.status().is_success());
            drop(reply);
        }
        Err(error) => {
            attempt.finish(false);
            value.detail = format!(
                "GET 兼容复核失败：{}；保留 HEAD 的 HTTPS 可达证据",
                failure(&error).1
            );
        }
    }
    value
}
async fn sample(
    state: &Lab,
    client: &reqwest::Client,
    at: DateTime<Utc>,
    controls: super::monitoring::Settings,
) -> anyhow::Result<()> {
    let slot = DateTime::from_timestamp(at.timestamp() / 180 * 180, 0).unwrap();
    let exists = tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM local_network_rounds WHERE slot=$1)",
        )
        .bind(slot)
        .fetch_one(&state.shared.pool),
    )
    .await??;
    if exists {
        return Ok(());
    }
    {
        let mut current = state
            .shared
            .local_round
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if current.started_at.is_none_or(|previous| previous <= at) {
            *current = LocalRound {
                started_at: Some(at),
                next_run_at: DateTime::from_timestamp((at.timestamp() / 180 + 1) * 180, 0),
                expected_sites: SITES.len(),
                ..Default::default()
            };
        }
    }
    let values = stream::iter(
        SITES
            .into_iter()
            .enumerate()
            .map(|(index, site)| async move {
                tokio::time::sleep(Duration::from_millis(index as u64 * 75)).await;
                let value = probe(client, site).await;
                let mut current = state
                    .shared
                    .local_round
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if current.started_at == Some(at) {
                    current.completed_sites += 1;
                }
                value
            }),
    )
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    let saved=tokio::time::timeout(Duration::from_secs(5),sqlx::query("INSERT INTO local_network_rounds(slot,checked_at,results) SELECT $1,$2,$3 WHERE EXISTS(SELECT 1 FROM proxy_lab_monitoring WHERE singleton AND enabled AND local_enabled AND global_epoch=$4 AND local_epoch=$5) ON CONFLICT(slot) DO NOTHING")
        .bind(slot).bind(at).bind(serde_json::to_value(values)?).bind(controls.global_epoch).bind(controls.local_epoch).execute(&state.shared.pool)).await??;
    let mut round = state
        .shared
        .local_round
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if round.started_at != Some(at) {
        return Ok(());
    }
    round.saved_sites = if saved.rows_affected() > 0 {
        SITES.len()
    } else {
        0
    };
    round.finished_at = Some(Utc::now());
    round.duration_ms = Some((Utc::now() - at).num_milliseconds().max(0) as u64);
    Ok(())
}
async fn run(state: Lab, client: reqwest::Client, at: DateTime<Utc>) {
    let Ok(initial) = super::monitoring::read(&state).await else {
        return;
    };
    if !initial.allows(true) {
        return;
    }
    let mut controls = state.shared.monitoring.subscribe();
    let completed = tokio::select! {biased;
        _=super::monitoring::interrupted(&mut controls,initial,true)=>{
            let mut round=state.shared.local_round.lock().unwrap_or_else(|e|e.into_inner());
            if round.started_at==Some(at){round.paused=true;round.finished_at=Some(Utc::now());round.next_run_at=None;}
            return;
        },
        result=tokio::time::timeout(Duration::from_secs(110),sample(&state,&client,at,initial))=>matches!(result,Ok(Ok(()))),
    };
    if !completed {
        let mut round = state
            .shared
            .local_round
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if round.started_at == Some(at) || round.started_at.is_none() {
            round.finished_at = Some(Utc::now());
            round.error = Some("本轮本机网络检测未完整保存；请核对平台数据库与本机连接资源".into());
        }
        tracing::warn!("local network sampling incomplete; no raw URLs or credentials logged");
    }
}
pub(super) fn start(state: Lab, ready: crate::database::Initialized) {
    if !enabled() {
        return;
    }
    tokio::spawn(async move {
        crate::database::wait_for_initialization(&ready).await;
        let Ok(client) = client() else { return };
        let initial = super::monitoring::read(&state).await.unwrap_or_default();
        let mut controls = state.shared.monitoring.subscribe();
        let mut previous = initial;
        let now = Utc::now();
        let delay = Duration::from_millis(
            (((now.timestamp() / 180 + 1) * 180 * 1000) - now.timestamp_millis()).max(1) as u64,
        );
        let mut clock = tokio::time::interval_at(
            tokio::time::Instant::now() + delay,
            Duration::from_secs(180),
        );
        clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut jobs = tokio::task::JoinSet::new();
        if initial.allows(true) {
            jobs.spawn(run(state.clone(), client.clone(), Utc::now()));
        }
        loop {
            tokio::select! {
                _=clock.tick()=>{jobs.spawn(run(state.clone(),client.clone(),Utc::now()));},
                changed=controls.changed()=>{
                    if changed.is_err(){return;}
                    let current=*controls.borrow_and_update();
                    if current.allows(true)&&(current.global_epoch!=previous.global_epoch||current.local_epoch!=previous.local_epoch){jobs.spawn(run(state.clone(),client.clone(),Utc::now()));}
                    previous=current;
                },
                _=jobs.join_next(),if !jobs.is_empty()=>{},
            }
        }
    });
}
fn local_series(
    value: (
        String,
        Option<history::Observation>,
        Option<DateTime<Utc>>,
        usize,
    ),
    count: usize,
) -> Value {
    let mut series = archive::series(value, count);
    series["legacy_samples"] = json!(0);
    series
}
async fn readings(
    state: &Lab,
    start: DateTime<Utc>,
    node: Option<Uuid>,
    limit: usize,
) -> Result<Vec<(i64, Value)>, Response> {
    let rows = if let Some(node) = node {
        sqlx::query("SELECT slot,jsonb_build_array(jsonb_path_query_first(results,'$[*] ? (@.id == $node)',jsonb_build_object('node',$2::text))) AS results FROM local_network_rounds WHERE slot>=$1 AND slot<=NOW() ORDER BY slot LIMIT $3")
            .bind(start).bind(node.to_string()).bind(limit as i64).fetch_all(&state.shared.pool).await.map_err(db)?
    } else {
        sqlx::query("SELECT slot,results FROM local_network_rounds WHERE slot>=$1 AND slot<=NOW() ORDER BY slot LIMIT $2")
        .bind(start).bind(limit as i64).fetch_all(&state.shared.pool).await.map_err(db)?
    };
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.get::<DateTime<Utc>, _>("slot").timestamp(),
                r.get("results"),
            )
        })
        .collect())
}
pub(super) async fn detail(State(state): State<Lab>) -> ApiResult<Value> {
    let monitoring = super::monitoring::read(&state).await.map_err(db)?;
    let now = Utc::now();
    let start = DateTime::from_timestamp(history::window(now), 0).unwrap();
    let ids: Vec<_> = SITES.into_iter().map(site_id).collect();
    let rounds = readings(&state, start, None, 480).await?;
    let mut values = history::histories(&ids, start.timestamp(), &rounds, false);
    let nodes: Vec<_> = SITES
        .into_iter()
        .map(|site| {
            let id = site_id(site);
            let mut value = local_series(values.remove(&id).unwrap(), 480);
            let fields = value.as_object_mut().unwrap();
            fields.insert("id".into(), json!(id));
            fields.insert("name".into(), json!(site.name));
            fields.insert("url".into(), json!(site.url));
            value
        })
        .collect();
    let round = state
        .shared
        .local_round
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    Ok(Json(
        json!({"source":SOURCE,"enabled":monitoring.allows(true),"configured_enabled":monitoring.local_enabled,"global_enabled":monitoring.enabled,"operator_enabled":enabled(),"monitoring":monitoring,"window_start":start,"generated_at":now,"slot_seconds":180,"check_seconds":180,"retention_days":7,"nodes":nodes,"round":round}),
    ))
}
pub(super) async fn overview(
    State(state): State<Lab>,
    Query(query): Query<archive::OverviewQuery>,
) -> ApiResult<Value> {
    let monitoring = super::monitoring::read(&state).await.map_err(db)?;
    let days = archive::days(query.days)?;
    let id = query.node.unwrap_or_else(|| site_id(SITES[0]));
    let site = find_site(id)?;
    let _permit = state
        .shared
        .history_reads
        .try_acquire()
        .map_err(|_| error(StatusCode::TOO_MANY_REQUESTS, "历史查询繁忙，请稍后重试"))?;
    let now = Utc::now();
    let start = archive::start(now, days);
    let count = days * 480;
    let values = readings(&state, start, Some(id), count).await?;
    let series = local_series(
        history::histories_with_slots(&[id], start.timestamp(), &values, false, count)
            .remove(&id)
            .unwrap(),
        count,
    );
    Ok(Json(
        json!({"source":SOURCE,"profile":{"id":"local-network","name":"本机网络检测","enabled":monitoring.allows(true)},"nodes":sites(),"selected_node":{"id":id,"name":site.name,"url":site.url,"protocol":"https","active":true},
        "generated_at":now,"window_start":start,"days":days,"slot_seconds":180,"retention_days":7,"https":series,"node_link":null}),
    ))
}
pub(super) async fn logs(
    State(state): State<Lab>,
    Query(query): Query<archive::LogsQuery>,
) -> ApiResult<Value> {
    let days = archive::days(query.days)?;
    find_site(query.node)?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(error(StatusCode::BAD_REQUEST, "每页日志数量应为 1–100"));
    }
    let _permit = state
        .shared
        .history_reads
        .try_acquire()
        .map_err(|_| error(StatusCode::TOO_MANY_REQUESTS, "历史查询繁忙，请稍后重试"))?;
    let now = Utc::now();
    let start = archive::start(now, days);
    let rows=sqlx::query("SELECT slot,checked_at,observation FROM (SELECT slot,checked_at,jsonb_path_query_first(results,'$[*] ? (@.id == $node)',jsonb_build_object('node',$2::text)) AS observation FROM local_network_rounds WHERE slot>=$1 AND slot<=NOW() AND ($3::TIMESTAMPTZ IS NULL OR slot<$3)) r WHERE observation IS NOT NULL ORDER BY slot DESC LIMIT $4")
        .bind(start).bind(query.node.to_string()).bind(query.before).bind(limit+1).fetch_all(&state.shared.pool).await.map_err(db)?;
    let more = rows.len() > limit as usize;
    let entries:Vec<_>=rows.into_iter().take(limit as usize).filter_map(|r|{
        let value=serde_json::from_value::<history::Observation>(r.get("observation")).ok()?;
        Some(json!({"slot":r.get::<DateTime<Utc>,_>("slot"),"checked_at":r.get::<DateTime<Utc>,_>("checked_at"),"revision":1,"observation":value}))}).collect();
    let next = if more {
        entries.last().map(|r| r["slot"].clone())
    } else {
        None
    };
    Ok(Json(
        json!({"source":SOURCE,"generated_at":now,"window_start":start,"days":days,"retention_days":7,"entries":entries,"has_more":more,"next_before":next}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[test]
    fn catalog_has_twenty_distinct_https_sites_and_stable_ids() {
        let ids: std::collections::HashSet<_> = SITES.into_iter().map(site_id).collect();
        let urls: std::collections::HashSet<_> = SITES.iter().map(|s| s.url).collect();
        assert_eq!(ids.len(), 20);
        assert_eq!(urls.len(), 20);
        assert!(
            SITES
                .iter()
                .all(|s| s.url.starts_with("https://") && !s.url.contains('@'))
        );
        assert_eq!(
            site_id(SITES[0]),
            site_id(Site {
                name: "renamed",
                url: SITES[0].url
            })
        );
        assert!(find_site(Uuid::new_v4()).is_err());
    }
    #[tokio::test]
    async fn valid_http_responses_keep_transport_reachability_and_actual_status_separate() {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        for status in [200, 302, 403, 429, 503] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url =
                Box::leak(format!("http://{}/", listener.local_addr().unwrap()).into_boxed_str());
            let task = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0; 4096];
                let size = socket.read(&mut bytes).await.unwrap();
                assert!(bytes[..size].starts_with(b"HEAD /"));
                socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: 0\r\nLocation: http://127.0.0.1:9/never-follow\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            });
            let value = probe(
                &client,
                Site {
                    name: "fixture",
                    url,
                },
            )
            .await;
            assert!(matches!(value.state.as_str(), "up" | "slow"));
            assert_eq!(value.http_status, Some(status));
            assert!(value.latency_ms.is_some());
            task.await.unwrap();
        }
    }
    #[tokio::test]
    async fn head_compatibility_keeps_both_statuses_without_reading_get_bodies() {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        for (head, get) in [
            (404, 200),
            (405, 200),
            (405, 302),
            (405, 403),
            (405, 429),
            (501, 200),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url =
                Box::leak(format!("http://{}/", listener.local_addr().unwrap()).into_boxed_str());
            let (release, wait) = tokio::sync::oneshot::channel::<()>();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0; 4096];
                let n = socket.read(&mut bytes).await.unwrap();
                assert!(bytes[..n].starts_with(b"HEAD /"));
                socket.write_all(format!("HTTP/1.1 {head} Fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
                drop(socket);
                let (mut socket, _) = listener.accept().await.unwrap();
                let n = socket.read(&mut bytes).await.unwrap();
                assert!(bytes[..n].starts_with(b"GET /"));
                // Never send the declared body. Reading it would time out and
                // fail this assertion; redirects and real 403/429 stay intact.
                socket.write_all(format!("HTTP/1.1 {get} Fixture\r\nContent-Length: 100000000\r\nLocation: http://127.0.0.1:9/never-follow\r\n\r\n").as_bytes()).await.unwrap();
                let _ = wait.await;
            });
            let value = tokio::time::timeout(
                Duration::from_secs(2),
                probe(
                    &client,
                    Site {
                        name: "fixture",
                        url,
                    },
                ),
            )
            .await
            .unwrap();
            assert_eq!(value.http_status, Some(get));
            assert!(matches!(value.state.as_str(), "up" | "slow"));
            let fallback = value.http_fallback.unwrap();
            assert_eq!(fallback.head_status, head);
            assert_eq!(fallback.get_status, Some(get));
            assert!(value.detail.is_empty());
            let _ = release.send(());
            server.await.unwrap();
        }
    }
    #[tokio::test]
    async fn failed_get_never_erases_a_verified_head_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Box::leak(format!("http://{}/", listener.local_addr().unwrap()).into_boxed_str());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            socket.read(&mut bytes).await.unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 405 Fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            drop(socket);
            let (mut socket, _) = listener.accept().await.unwrap();
            socket.read(&mut bytes).await.unwrap();
            drop(socket);
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(500))
            .build()
            .unwrap();
        let value = probe(
            &client,
            Site {
                name: "fixture",
                url,
            },
        )
        .await;
        assert_eq!(value.http_status, Some(405));
        assert!(matches!(value.state.as_str(), "up" | "slow"));
        assert!(value.http_fallback.unwrap().get_status.is_none());
        assert!(value.detail.contains("保留 HEAD"));
        server.await.unwrap();
    }
    #[tokio::test]
    async fn connection_failures_are_not_fabricated_as_http_responses() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(100))
            .build()
            .unwrap();
        let url = Box::leak(format!("http://{address}/").into_boxed_str());
        let value = probe(
            &client,
            Site {
                name: "fixture",
                url,
            },
        )
        .await;
        assert_eq!(value.state, "down");
        assert!(value.http_status.is_none());
        assert!(value.latency_ms.is_none());
    }
    #[tokio::test]
    async fn production_client_never_accepts_plaintext_targets() {
        assert!(
            client()
                .unwrap()
                .head("http://127.0.0.1:9/")
                .send()
                .await
                .is_err()
        );
    }
}
