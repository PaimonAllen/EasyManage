use super::*;
use futures_util::{FutureExt, StreamExt, stream};
use serde::Serialize;
use std::time::Duration;

#[derive(Clone, Serialize)]
pub struct Job {
    pub id: Uuid,
    pub status: String,
    pub total: usize,
    pub completed: usize,
    pub message: String,
    pub started_at: DateTime<Utc>,
}
#[derive(Clone, Default, Serialize)]
pub struct RoundStatus {
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub expected_nodes: usize,
    pub completed_nodes: usize,
    pub saved_nodes: usize,
    pub timed_out_nodes: usize,
    pub next_run_at: Option<DateTime<Utc>>,
    pub profile_ids: Vec<Uuid>,
    #[serde(skip)]
    token: Uuid,
}
struct Flag {
    state: Lab,
    score: bool,
    batch: Option<Uuid>,
}
impl Drop for Flag {
    fn drop(&mut self) {
        if self.score {
            self.state.shared.score_busy.store(false, Ordering::SeqCst);
        } else {
            let mut round = self
                .state
                .shared
                .round
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if Some(round.token) != self.batch {
                return;
            }
            round.finished_at = Some(Utc::now());
            round.duration_ms = round
                .started_at
                .map(|at| (Utc::now() - at).num_milliseconds().max(0) as u64);
        }
    }
}
#[derive(Clone)]
struct Pending {
    id: Uuid,
    revision: i64,
}
fn next_boundary(now: DateTime<Utc>) -> Duration {
    let next = (now.timestamp() / 180 + 1) * 180;
    Duration::from_millis((next * 1000 - now.timestamp_millis()).max(1) as u64)
}
async fn due(state: &Lab, at: DateTime<Utc>) -> anyhow::Result<Vec<Pending>> {
    if !super::monitoring::read(state).await?.enabled {
        return Ok(vec![]);
    }
    let slot = DateTime::from_timestamp(at.timestamp() / 180 * 180, 0).unwrap();
    let rows=sqlx::query("SELECT p.id,p.revision FROM proxy_lab_profiles p WHERE p.enabled AND EXISTS(SELECT 1 FROM proxy_lab_monitoring WHERE singleton AND enabled) AND NOT EXISTS(SELECT 1 FROM proxy_lab_rounds r WHERE r.profile_id=p.id AND r.slot=$1 AND r.revision=p.revision AND COALESCE(r.results->0->>'policy','1')=$2) ORDER BY p.created_at,p.id")
        .bind(slot).bind(history::POLICY.to_string()).fetch_all(&state.shared.pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| Pending {
            id: r.get("id"),
            revision: r.get("revision"),
        })
        .collect())
}
pub(super) type FlightKey = (Uuid, i64, i64);
struct InFlight {
    keys: Vec<FlightKey>,
    active: Arc<std::sync::Mutex<std::collections::HashSet<FlightKey>>>,
}
impl Drop for InFlight {
    fn drop(&mut self) {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        for key in &self.keys {
            active.remove(key);
        }
    }
}
pub fn start(state: Lab, ready: crate::database::Initialized) {
    tokio::spawn(async move {
        crate::database::wait_for_initialization(&ready).await;
        let active = state.shared.in_flight.clone();
        let mut tasks = tokio::task::JoinSet::new();
        let mut ticks = tokio::time::interval_at(
            tokio::time::Instant::now() + next_boundary(Utc::now()),
            Duration::from_secs(180),
        );
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut maintenance = tokio::time::Instant::now();
        loop {
            if tokio::time::Instant::now() >= maintenance {
                state.shared.vault.cleanup_stale_workspaces();
                let _ = history::prune(&state.shared.pool).await;
                maintenance = tokio::time::Instant::now() + Duration::from_secs(900);
            }
            let at = Utc::now();
            let slot = at.timestamp() / 180;
            let mut storage_retry = false;
            match due(&state, at).await {
                Ok(profiles) => {
                    let mut guard = active.lock().unwrap_or_else(|e| e.into_inner());
                    let profiles: Vec<_> = profiles
                        .into_iter()
                        .filter(|p| guard.insert((p.id, p.revision, slot)))
                        .collect();
                    let keys = profiles.iter().map(|p| (p.id, p.revision, slot)).collect();
                    drop(guard);
                    if !profiles.is_empty() {
                        let state = state.clone();
                        let active = active.clone();
                        tasks.spawn(async move {
                            let _flight = InFlight { keys, active };
                            let initial=*state.shared.monitoring.borrow();
                            let mut controls=state.shared.monitoring.subscribe();
                            tokio::select!{biased;
                                _=super::monitoring::interrupted(&mut controls,initial,false)=>Ok(()),
                                result=check_all(&state, profiles, at)=>result,
                            }
                        });
                    }
                }
                Err(_) => storage_retry = true,
            }
            // The sampling clock never awaits a probe batch. Slow earlier batches
            // can finish independently; slot/profile/revision keys prevent duplicates.
            if matches!(
                scheduler_wake(&mut ticks, &state.shared.notify, &mut tasks, storage_retry).await,
                SchedulerWake::BatchFailed
            ) {
                tracing::warn!("platform proxy batch incomplete; no credentials logged");
                storage_retry = true;
            }
            if storage_retry {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    });
}
enum SchedulerWake {
    Tick,
    Changed,
    BatchFinished,
    BatchFailed,
    Retry,
}
async fn scheduler_wake(
    ticks: &mut tokio::time::Interval,
    notify: &Arc<Notify>,
    tasks: &mut tokio::task::JoinSet<anyhow::Result<()>>,
    storage_retry: bool,
) -> SchedulerWake {
    tokio::select! {
        _=ticks.tick()=>SchedulerWake::Tick,
        _=notify.clone().notified_owned()=>SchedulerWake::Changed,
        result=tasks.join_next(),if !tasks.is_empty()=>if matches!(result,Some(Ok(Ok(())))){SchedulerWake::BatchFinished}else{SchedulerWake::BatchFailed},
        _=tokio::time::sleep(Duration::from_secs(15)),if storage_retry=>SchedulerWake::Retry,
    }
}
fn start_offset(id: Uuid, slot: i64, window: Duration) -> Duration {
    let width = window.as_millis() as u64;
    if width == 0 {
        return Duration::ZERO;
    }
    // SplitMix64 avoids clustered UUID prefixes; shift each sampling period.
    let mut value = (id.as_u128() as u64) ^ ((id.as_u128() >> 64) as u64) ^ slot as u64;
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    Duration::from_millis((value ^ (value >> 31)) % width)
}
const NODE_WEBSITE_TIMEOUT: Duration = Duration::from_secs(100);
async fn website_with_deadline<F>(
    id: Uuid,
    budget: Duration,
    website: F,
) -> (history::Observation, bool)
where
    F: std::future::Future<Output = history::Observation>,
{
    match tokio::time::timeout(budget, website).await {
        Ok(value) => (value, false),
        Err(_) => (
            unknown(id, "本周期网站探测超时，已记录未确认；下一周期继续检查"),
            true,
        ),
    }
}
async fn collect_measurements<F, L>(
    id: Uuid,
    budget: Duration,
    website: F,
    link: L,
) -> (history::Observation, bool)
where
    F: std::future::Future<Output = history::Observation>,
    L: std::future::Future<Output = history::NodeLink>,
{
    let ((mut value, expired), link) =
        tokio::join!(website_with_deadline(id, budget, website), link);
    value.link = Some(link);
    (value, expired)
}
fn round_progress(state: &Lab, token: Uuid, completed: usize, saved: usize, timed_out: usize) {
    let mut round = state.shared.round.lock().unwrap_or_else(|e| e.into_inner());
    if round.token == token {
        round.completed_nodes += completed;
        round.saved_nodes += saved;
        round.timed_out_nodes += timed_out;
    }
}
async fn save(
    state: &Lab,
    id: Uuid,
    revision: i64,
    checked_at: DateTime<Utc>,
    observations: Vec<history::Observation>,
    global_epoch: i64,
) -> anyhow::Result<usize> {
    let count = observations.len();
    let slot = DateTime::from_timestamp(checked_at.timestamp() / 180 * 180, 0).unwrap();
    let query=sqlx::query("INSERT INTO proxy_lab_rounds(profile_id,slot,checked_at,revision,results) SELECT $1,$2,$3,$4,$5 WHERE EXISTS(SELECT 1 FROM proxy_lab_profiles WHERE id=$1 AND enabled AND revision=$4) AND EXISTS(SELECT 1 FROM proxy_lab_monitoring WHERE singleton AND enabled AND global_epoch=$7) ON CONFLICT(profile_id,slot) DO UPDATE SET checked_at=EXCLUDED.checked_at,revision=EXCLUDED.revision,results=EXCLUDED.results WHERE proxy_lab_rounds.revision<>EXCLUDED.revision OR COALESCE(proxy_lab_rounds.results->0->>'policy','1')<>$6")
        .bind(id).bind(slot).bind(checked_at).bind(revision).bind(serde_json::to_value(observations)?).bind(history::POLICY.to_string()).bind(global_epoch).execute(&state.shared.pool);
    let result = tokio::time::timeout(Duration::from_secs(5), query).await??;
    Ok(if result.rows_affected() > 0 { count } else { 0 })
}
fn unknown(id: Uuid, message: &str) -> history::Observation {
    history::Observation {
        id,
        state: "unknown".into(),
        latency_ms: None,
        http_status: None,
        http_fallback: None,
        detail: message.into(),
        policy: history::POLICY,
        sites: vec![],
        link: Some(history::NodeLink {
            state: "unknown".into(),
            latency_ms: None,
            detail: message.into(),
            kind: "tcp".into(),
        }),
        site_feedback: vec![],
    }
}
struct Batch {
    profile: Pending,
    global_epoch: i64,
    epoch: watch::Receiver<u64>,
    nodes: Vec<RuntimeNode>,
}
async fn check_all(state: &Lab, profiles: Vec<Pending>, at: DateTime<Utc>) -> anyhow::Result<()> {
    let global_epoch = state.shared.monitoring.borrow().global_epoch;
    let token = Uuid::new_v4();
    {
        let mut round = state.shared.round.lock().unwrap_or_else(|e| e.into_inner());
        if round.started_at.is_none_or(|previous| previous <= at) {
            *round = RoundStatus {
                token,
                next_run_at: DateTime::from_timestamp((at.timestamp() / 180 + 1) * 180, 0),
                started_at: Some(at),
                profile_ids: profiles.iter().map(|p| p.id).collect(),
                ..Default::default()
            };
        }
    }
    let _flag = Flag {
        state: state.clone(),
        score: false,
        batch: Some(token),
    };
    let mut batches = Vec::new();
    let mut unavailable_nodes = 0;
    for profile in profiles {
        let epoch = state.profile_generation(profile.id);
        match state.load_nodes(profile.id, None).await {
            Ok(nodes)
                if !nodes.is_empty()
                    && nodes.iter().all(|n| n.revision == profile.revision)
                    && !epoch.has_changed().unwrap_or(true) =>
            {
                batches.push(Batch {
                    profile,
                    global_epoch,
                    epoch,
                    nodes,
                })
            }
            Ok(_) => {}
            Err(_) => {
                let ids = sqlx::query_scalar::<_, Uuid>(
                    "SELECT id FROM proxy_lab_nodes WHERE profile_id=$1 AND active",
                )
                .bind(profile.id)
                .fetch_all(&state.shared.pool)
                .await?;
                if !epoch.has_changed().unwrap_or(true) {
                    unavailable_nodes += ids.len();
                    let saved = save(
                        state,
                        profile.id,
                        profile.revision,
                        at,
                        ids.into_iter()
                            .map(|id| {
                                unknown(id, "平台无法读取代理凭据；请检查私有主密钥，原数据已保留")
                            })
                            .collect(),
                        global_epoch,
                    )
                    .await?;
                    round_progress(state, token, 0, saved, 0);
                }
            }
        }
    }
    {
        let mut round = state.shared.round.lock().unwrap_or_else(|e| e.into_inner());
        if round.token == token {
            round.expected_nodes =
                unavailable_nodes + batches.iter().map(|b| b.nodes.len()).sum::<usize>();
            round.completed_nodes = unavailable_nodes;
        }
    }
    let runnable: Vec<_> = batches
        .iter()
        .flat_map(|b| b.nodes.iter())
        .filter(|n| n.unsupported.is_none())
        .map(|n| (n.id, n.definition.clone()))
        .collect();
    // DNS is deduplicated by hostname. One isolated core serves the entire fleet;
    // nodes and profiles run independently; each node paces its own website requests.
    let spread = Duration::from_secs(if runnable.len() > 64 { 50 } else { 0 });
    let prepared = engine::prepare(&runnable).await;
    let mut core = if runnable.is_empty() {
        None
    } else {
        engine::Engine::from_prepared(&state.shared.vault, &prepared, false)
            .await
            .ok()
    };
    let tasks = batches
        .into_iter()
        .map(|batch| check_profile(state, batch, at, token, &prepared, core.as_ref(), spread));
    for result in futures_util::future::join_all(tasks).await {
        result?;
    }
    if let Some(engine) = core.as_mut() {
        engine.shutdown().await;
    }
    Ok(())
}
fn measurement_key(definition: &Value) -> Vec<u8> {
    let mut definition = definition.clone();
    if let Some(fields) = definition.as_object_mut() {
        fields.remove("name");
    }
    ring::digest::digest(
        &ring::digest::SHA256,
        &serde_json::to_vec(&definition).unwrap_or_default(),
    )
    .as_ref()
    .to_vec()
}
fn merge_feedback(target: &mut Vec<history::SiteFeedback>, values: &[history::SiteFeedback]) {
    for value in values {
        if let Some(previous) = target.iter_mut().find(|p| p.site == value.site) {
            if value.retry_at > previous.retry_at {
                previous.retry_at = value.retry_at;
                previous.primary_status = value.primary_status;
            }
            if value.fallback_retry_at > previous.fallback_retry_at {
                previous.fallback_retry_at = value.fallback_retry_at;
                previous.fallback_status = value.fallback_status;
            }
            previous.rate_limited |= value.rate_limited;
        } else {
            target.push(value.clone());
        }
    }
}
async fn check_profile(
    state: &Lab,
    mut batch: Batch,
    at: DateTime<Utc>,
    token: Uuid,
    prepared: &engine::Prepared,
    core: Option<&engine::Engine>,
    spread: Duration,
) -> anyhow::Result<()> {
    if batch.epoch.has_changed().unwrap_or(true) {
        return Ok(());
    }
    // A single malformed protocol must not block unrelated configurations. The
    // normal path shares one core; startup failure falls back to profile isolation.
    let mut fallback = if core.is_none() && batch.nodes.iter().any(|n| n.unsupported.is_none()) {
        let ids = batch.nodes.iter().map(|n| n.id).collect();
        engine::Engine::from_prepared(&state.shared.vault, &prepared.subset(&ids), false)
            .await
            .ok()
    } else {
        None
    };
    let core = core.or(fallback.as_ref());
    // Provider YAMLs often repeat identical credentials under multiple names.
    // Share a real in-round measurement for exact aliases in this configuration;
    // every alias still receives its own timestamped website and link records.
    let mut feedback_by_key: HashMap<Vec<u8>, Vec<history::SiteFeedback>> = HashMap::new();
    for node in &batch.nodes {
        merge_feedback(
            feedback_by_key
                .entry(measurement_key(&node.definition))
                .or_default(),
            &node.site_feedback,
        );
    }
    let mut shared_probes = HashMap::new();
    let nodes = batch.nodes.into_iter().map(|mut node| {
        let id = node.id;
        let key = measurement_key(&node.definition);
        node.site_feedback = feedback_by_key.get(&key).cloned().unwrap_or_default();
        let probe = shared_probes
            .entry(key)
            .or_insert_with(|| {
                async move {
                    // Launch every node asynchronously, with a bounded start window to avoid
                    // a fleet-wide handshake burst against shared exits and local resources.
                    tokio::time::sleep(start_offset(node.id, at.timestamp() / 180, spread)).await;
                    let mut timed_out = false;
                    let mut value = if let Some(message) = node.unsupported {
                        let mut value = unknown(node.id, &message);
                        value.state = "unsupported".into();
                        value.link = Some(history::NodeLink {
                            state: "unsupported".into(),
                            latency_ms: None,
                            detail: message,
                            kind: "unknown".into(),
                        });
                        value
                    } else {
                        let website = async {
                            match core {
                                Some(engine) => {
                                    engine
                                        .check_with_feedback(node.id, node.site_feedback.clone())
                                        .await
                                }
                                None => unknown(node.id, "独立检测内核不可用；不计为网站故障"),
                            }
                        };
                        let (mut value, expired) = collect_measurements(
                            node.id,
                            NODE_WEBSITE_TIMEOUT,
                            website,
                            prepared.link(node.id),
                        )
                        .await;
                        timed_out = expired;
                        if expired {
                            value.site_feedback = node
                                .site_feedback
                                .iter()
                                .filter(|f| {
                                    f.retry_at.is_some_and(|at| at > Utc::now())
                                        || f.fallback_retry_at.is_some_and(|at| at > Utc::now())
                                })
                                .cloned()
                                .collect();
                        }
                        value
                    };
                    if let Some(note) = prepared.dns_note(node.id) {
                        if !value.detail.is_empty() {
                            value.detail.push_str("；");
                        }
                        value.detail.push_str(note);
                    }
                    // Successful expected-status samples already retain their verified
                    // milliseconds. Keep additional feedback only for errors, fallback
                    // use or active cooldowns, rather than duplicate normal log strings.
                    value.site_feedback.retain(|f| {
                        f.outcome != "primary_ok"
                            || f.retry_at.is_some()
                            || f.fallback_retry_at.is_some()
                    });
                    value.policy = history::POLICY;
                    (value, timed_out)
                }
                .boxed()
                .shared()
            })
            .clone();
        async move {
            let (mut value, timed_out) = probe.await;
            value.id = id;
            round_progress(state, token, 1, 0, usize::from(timed_out));
            value
        }
    });
    let observations = tokio::select! {
        biased;
        _=batch.epoch.changed()=>return Ok(()),
        result=futures_util::future::join_all(nodes)=>result,
    };
    drop(shared_probes);
    if batch.epoch.has_changed().unwrap_or(true) {
        return Ok(());
    }
    let saved = save(
        state,
        batch.profile.id,
        batch.profile.revision,
        at,
        observations,
        batch.global_epoch,
    )
    .await;
    if let Ok(count) = saved.as_ref() {
        round_progress(state, token, 0, *count, 0);
    }
    if let Some(core) = fallback.as_mut() {
        core.shutdown().await;
    }
    saved.map(|_| ())
}
#[cfg(test)]
mod queue_tests {
    use super::*;
    #[tokio::test]
    async fn the_next_sampling_tick_does_not_wait_for_an_unfinished_batch() {
        let mut tasks = tokio::task::JoinSet::<anyhow::Result<()>>::new();
        tasks.spawn(async {
            std::future::pending::<()>().await;
            Ok(())
        });
        let mut ticks = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_millis(10),
            Duration::from_millis(10),
        );
        let notify = Arc::new(Notify::new());
        let wake = tokio::time::timeout(
            Duration::from_millis(100),
            scheduler_wake(&mut ticks, &notify, &mut tasks, false),
        )
        .await
        .unwrap();
        assert!(matches!(wake, SchedulerWake::Tick));
        assert_eq!(tasks.len(), 1);
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
    #[tokio::test]
    async fn a_hung_website_gets_a_real_unknown_record_without_losing_the_tcp_result() {
        let id = Uuid::new_v4();
        let link = async {
            history::NodeLink {
                state: "up".into(),
                latency_ms: Some(17),
                detail: "connected".into(),
                kind: "tcp".into(),
            }
        };
        let (value, expired) =
            collect_measurements(id, Duration::from_millis(5), std::future::pending(), link).await;
        assert!(expired);
        assert_eq!(value.id, id);
        assert_eq!(value.state, "unknown");
        assert_eq!(value.link.unwrap().latency_ms, Some(17));
    }
    #[test]
    fn asynchronous_start_offsets_spread_a_fleet_inside_each_period() {
        let id = Uuid::new_v4();
        let window = Duration::from_secs(50);
        assert_eq!(start_offset(id, 10, Duration::ZERO), Duration::ZERO);
        assert_eq!(start_offset(id, 10, window), start_offset(id, 10, window));
        assert_ne!(start_offset(id, 10, window), start_offset(id, 11, window));
        let buckets: std::collections::HashSet<_> = (0..367)
            .map(|_| start_offset(Uuid::new_v4(), 10, window).as_secs())
            .collect();
        assert!(buckets.len() > 40);
        assert!(buckets.iter().all(|v| *v < 50));
        assert!(window + NODE_WEBSITE_TIMEOUT + Duration::from_secs(20) < Duration::from_secs(180));
    }
    #[test]
    fn cadence_is_aligned_to_three_minute_slots_not_round_completion() {
        assert_eq!(
            next_boundary(DateTime::from_timestamp(360, 0).unwrap()).as_secs(),
            180
        );
        assert_eq!(
            next_boundary(DateTime::from_timestamp(535, 0).unwrap()).as_secs(),
            5
        );
        assert_eq!(
            next_boundary(DateTime::from_timestamp(539, 500_000_000).unwrap()).as_millis(),
            500
        );
    }
}
pub async fn create_job(state: &Lab, total: usize) -> Uuid {
    let id = Uuid::new_v4();
    let mut jobs = state.shared.jobs.lock().await;
    if jobs.len() >= 16 {
        if let Some(oldest) = jobs
            .values()
            .filter(|j| j.status != "running")
            .min_by_key(|j| j.started_at)
            .map(|j| j.id)
        {
            jobs.remove(&oldest);
        }
    }
    jobs.insert(
        id,
        Job {
            id,
            status: "running".into(),
            total,
            completed: 0,
            message: "准备独立代理出口；评分仅由本次手动操作发起".into(),
            started_at: Utc::now(),
        },
    );
    id
}
async fn progress(state: &Lab, id: Uuid, completed: usize, message: &str, status: &str) {
    if let Some(job) = state.shared.jobs.lock().await.get_mut(&id) {
        job.completed = completed;
        job.message = message.into();
        job.status = status.into();
    }
}
pub fn start_score(state: Lab, profile: Uuid, selected: Vec<Uuid>, job: Uuid) {
    tokio::spawn(async move {
        let mut epoch = state.profile_generation(profile);
        let initial = *state.shared.monitoring.borrow();
        let mut controls = state.shared.monitoring.subscribe();
        let _flag = Flag {
            state: state.clone(),
            score: true,
            batch: None,
        };
        let result = tokio::select! {
            biased;
            _=super::monitoring::interrupted(&mut controls,initial,false)=>{
                let done=state.shared.jobs.lock().await.get(&job).map_or(0,|j|j.completed);
                progress(&state,job,done,"所有检测已暂停，本次手动评分已取消；历史结果保留","cancelled").await;return;
            },
            _=epoch.changed()=>{
                let done=state.shared.jobs.lock().await.get(&job).map_or(0,|j|j.completed);
                progress(&state,job,done,"配置已删除、暂停或更新，本次评分已取消；其他配置不受影响","cancelled").await;
                return;
            },
            result=tokio::time::timeout(Duration::from_secs(620),execute_score(&state,profile,&selected,job))=>result,
        };
        match result {
            Ok(Ok(())) => {
                progress(
                    &state,
                    job,
                    selected.len(),
                    "本次手动检测完成；受限来源保留状态，可打开原站核验",
                    "success",
                )
                .await
            }
            _ => {
                let done = state
                    .shared
                    .jobs
                    .lock()
                    .await
                    .get(&job)
                    .map_or(0, |j| j.completed);
                progress(
                    &state,
                    job,
                    done,
                    "检测未完成；已取得的结果保留。请检查检测内核、配置版本或平台连接",
                    "failed",
                )
                .await;
            }
        }
    });
}
async fn execute_score(
    state: &Lab,
    profile: Uuid,
    selected: &[Uuid],
    job: Uuid,
) -> anyhow::Result<()> {
    let nodes = state.load_nodes(profile, Some(selected)).await?;
    if nodes.len() != selected.len() {
        anyhow::bail!("selected nodes no longer exist");
    }
    let runnable: Vec<_> = nodes
        .iter()
        .filter(|n| n.unsupported.is_none())
        .map(|n| (n.id, n.definition.clone()))
        .collect();
    let mut engine = engine::Engine::start(&state.shared.vault, &runnable, true).await?;
    for (index, node) in nodes.iter().enumerate() {
        let revision = sqlx::query_scalar::<_, i64>(
            "SELECT revision FROM proxy_lab_nodes WHERE id=$1 AND active",
        )
        .bind(node.id)
        .fetch_optional(&state.shared.pool)
        .await?;
        if revision != Some(node.revision) {
            anyhow::bail!("configuration changed");
        }
        progress(
            state,
            job,
            index,
            "通过所选节点确认出口 IP，并查询可用机构接口",
            "running",
        )
        .await;
        let client = if node.unsupported.is_none() {
            engine.score_client(node.id).await.ok()
        } else {
            None
        };
        if let Some(client) = client {
            match score::egress(&client).await {
                Ok(ip) => {
                    let futures: Vec<_> = score::PROVIDERS
                        .into_iter()
                        .map(|provider| {
                            let client = client.clone();
                            async move { score::query(&client, &provider, ip).await }
                        })
                        .collect();
                    let mut queries = stream::iter(futures).buffer_unordered(3);
                    while let Some(value) = queries.next().await {
                        state.save_score(node.id, node.revision, &value).await?;
                    }
                }
                Err(reason) => {
                    for provider in &score::PROVIDERS {
                        state
                            .save_score(
                                node.id,
                                node.revision,
                                &score::Score::unavailable(provider.id, "unavailable", &reason),
                            )
                            .await?;
                    }
                }
            }
        } else {
            for provider in &score::PROVIDERS {
                state
                    .save_score(
                        node.id,
                        node.revision,
                        &score::Score::unavailable(
                            provider.id,
                            "unsupported",
                            node.unsupported
                                .as_deref()
                                .unwrap_or("无法建立该节点的独立检测出口"),
                        ),
                    )
                    .await?;
            }
        }
        progress(
            state,
            job,
            index + 1,
            "来源结果已保存，未取得的分数不会按零分处理",
            "running",
        )
        .await;
    }
    engine.shutdown().await;
    Ok(())
}
