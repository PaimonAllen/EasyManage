//! Ephemeral directory trees. Never serialize these trees to PostgreSQL or disk.
use super::*;
use std::collections::BTreeMap;

const RETENTION: Duration = Duration::from_hours(24);
const VIEWER_LEASE: Duration = Duration::from_secs(90);
pub(super) type Job = Arc<Mutex<LiveScan>>;

pub(super) struct LiveScan {
    pub node: Uuid,
    pub summary: Value,
    metadata: Value,
    directories: BTreeMap<i64, Value>,
    children: HashMap<i64, Vec<i64>>,
    viewers: HashMap<Uuid, Instant>,
    idle_since: Option<Instant>,
    stop: Arc<AtomicBool>,
    revision: u64,
}

impl LiveScan {
    pub fn new(node: Uuid, id: Uuid, request: &Request, stop: Arc<AtomicBool>) -> Self {
        let now = Instant::now();
        Self {
            node,
            summary: json!({"id":id,"mode":request.mode,"scope":request.scope,
                "storage":"realtime","root_path":request.path,"status":"running",
                "started_at":Utc::now(),"finished_at":null,"progress":{},"message":""}),
            metadata: Value::Null,
            directories: BTreeMap::new(),
            children: HashMap::new(),
            viewers: request
                .viewer
                .map(|id| HashMap::from([(id, now)]))
                .unwrap_or_default(),
            idle_since: request.viewer.is_none().then_some(now),
            stop,
            revision: 0,
        }
    }

    // Missing pagehide/beacon (crash, network loss) is covered by the lease.
    fn expire_viewers(&mut self, now: Instant) {
        let last = self.viewers.values().copied().max();
        self.viewers
            .retain(|_, seen| now.saturating_duration_since(*seen) < VIEWER_LEASE);
        if self.viewers.is_empty() && self.idle_since.is_none() {
            self.idle_since = Some(last.map_or(now, |seen| seen + VIEWER_LEASE));
        }
    }

    pub fn expired(&mut self, now: Instant) -> bool {
        self.expire_viewers(now);
        self.idle_since
            .is_some_and(|since| now.saturating_duration_since(since) >= RETENTION)
    }

    pub fn presence(&mut self, viewer: Uuid, attached: bool, now: Instant) {
        self.expire_viewers(now);
        if attached {
            self.viewers.insert(viewer, now);
            self.idle_since = None;
        } else if self.viewers.remove(&viewer).is_some() && self.viewers.is_empty() {
            self.idle_since = Some(now);
        }
    }

    pub fn summary(&self) -> Value {
        let mut value = self.summary.clone();
        value.as_object_mut().unwrap().remove("result");
        value["revision"] = json!(self.revision);
        value["expires_at"] = self
            .idle_since
            .map(|since| {
                let remaining = RETENTION.saturating_sub(since.elapsed());
                json!(Utc::now() + chrono::Duration::from_std(remaining).unwrap_or_default())
            })
            .unwrap_or(Value::Null);
        value
    }

    pub fn detail(&self) -> Value {
        let mut value = self.summary();
        value["result"] = self.summary["result"].clone();
        if value["result"].is_null() && !self.metadata.is_null() {
            if let Some(root) = self.directories.get(&0) {
                let mut result = self.metadata.clone();
                for (key, default) in [
                    ("visited", 0),
                    ("errors", 0),
                    ("skipped_mounts", 0),
                    ("symlinks", 0),
                    ("hardlinks", 0),
                ] {
                    result[key] = value["progress"][key].clone();
                    if result[key].is_null() {
                        result[key] = json!(default);
                    }
                }
                result["elapsed_seconds"] = value["progress"]["elapsed_seconds"]
                    .as_f64()
                    .map_or(json!(0), |n| json!(n));
                result["status"] = value["status"].clone();
                result["reason"] = json!(if value["status"] == "running" {
                    "scanning"
                } else {
                    "connection_lost"
                });
                result["filesystem"] = json!("scanning");
                result["nodes"] = json!([root]);
                result["paged"] = json!(true);
                result["detail_level"] = json!("directories");
                result["saved_directories"] = json!(self.directories.len());
                value["result"] = result;
            }
        }
        if !value["result"].is_null() {
            value["result"]["revision"] = json!(self.revision);
        }
        value
    }

    pub fn metadata(&mut self, value: &Value) {
        self.metadata = value.clone();
    }
    pub fn control_changed(&mut self) { self.revision += 1; }
    pub fn progress(&mut self, value: &Value) {
        self.summary["progress"] = value.clone();
        self.revision += 1;
    }
    pub fn insert(&mut self, nodes: &[Value]) -> Result<(), OperationsError> {
        for node in nodes {
            let id = node["id"].as_i64().ok_or_else(OperationsError::registry)?;
            if let Some(old) = self.directories.get(&id) {
                if ["parent", "name", "kind", "scan_path"]
                    .iter()
                    .any(|key| old[*key] != node[*key])
                {
                    return Err(OperationsError::ssh("目录条目身份发生变化"));
                }
            } else if let Some(parent) = node["parent"].as_i64() {
                self.children.entry(parent).or_default().push(id);
            }
            self.directories.insert(id, node.clone());
        }
        self.revision += 1;
        Ok(())
    }
    pub fn finish(&mut self, status: &str, result: Option<Value>, message: &str) {
        self.summary["status"] = json!(status);
        self.summary["result"] = result.unwrap_or(Value::Null);
        self.summary["message"] = json!(message);
        self.summary["finished_at"] = json!(Utc::now());
        self.revision += 1;
    }

    pub fn page(&self, query: &EntryQuery) -> Result<Value, OperationsError> {
        let directory = self.directories.get(&query.parent).ok_or(OperationsError(
            StatusCode::NOT_FOUND,
            "目录尚未扫描或不存在",
        ))?;
        let mut ancestors = vec![directory.clone()];
        while let Some(parent) = ancestors.last().and_then(|n| n["parent"].as_i64()) {
            let Some(entry) = self.directories.get(&parent) else {
                break;
            };
            ancestors.push(entry.clone());
            if ancestors.len() > 66 {
                break;
            }
        }
        ancestors.reverse();
        let children: Vec<&Value> = self
            .children
            .get(&query.parent)
            .into_iter()
            .flatten()
            .filter_map(|id| self.directories.get(id))
            .collect();
        let direct: serde_json::Map<String, Value> = ["allocated", "apparent", "files"]
            .into_iter()
            .map(|key| {
                let sum = children.iter().fold(0_i64, |sum, n| {
                    sum.saturating_add(n[key].as_i64().unwrap_or(0))
                });
                (
                    key.to_owned(),
                    json!(
                        directory[key]
                            .as_i64()
                            .unwrap_or(0)
                            .saturating_sub(sum)
                            .max(0)
                    ),
                )
            })
            .collect();
        let search = query.search.to_lowercase();
        let mut matching: Vec<&Value> = children
            .iter()
            .copied()
            .filter(|n| {
                n["name"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&search)
            })
            .collect();
        let key = match query.sort.as_str() {
            "name" => "name",
            "files" => "files",
            _ => &query.basis,
        };
        matching.sort_unstable_by(|a, b| {
            let order = if key == "name" {
                a[key].as_str().cmp(&b[key].as_str())
            } else {
                a[key].as_i64().cmp(&b[key].as_i64())
            };
            (if query.ascending {
                order
            } else {
                order.reverse()
            })
            .then_with(|| a["id"].as_i64().cmp(&b["id"].as_i64()))
        });
        let total = matching.len();
        let entries: Vec<&Value> = matching
            .into_iter()
            .skip(usize::try_from(query.offset).unwrap_or(usize::MAX))
            .take(250)
            .collect();
        let mut top = children;
        top.retain(|n| n[&query.basis].as_i64().unwrap_or(0) > 0);
        top.sort_unstable_by(|a, b| {
            b[&query.basis]
                .as_i64()
                .cmp(&a[&query.basis].as_i64())
                .then_with(|| a["id"].as_i64().cmp(&b["id"].as_i64()))
        });
        top.truncate(40);
        Ok(
            json!({"directory":directory,"ancestors":ancestors,"entries":entries,"top":top,
            "direct":direct,"total":total,"offset":query.offset,"limit":250,"revision":self.revision}),
        )
    }
}

impl Scans {
    pub(super) fn live_job(&self, node: Uuid, id: Uuid) -> Result<Option<Job>, OperationsError> {
        self.reap_live()?;
        let jobs = self.live.lock().map_err(|_| OperationsError::busy())?;
        Ok(jobs
            .get(&id)
            .filter(|job| job.lock().is_ok_and(|job| job.node == node))
            .cloned())
    }
    pub(super) fn reap_live(&self) -> Result<(), OperationsError> {
        let now = Instant::now();
        self.live
            .lock()
            .map_err(|_| OperationsError::busy())?
            .retain(|_, job| {
                job.lock().map_or(true, |mut job| {
                    if job.expired(now) {
                        job.stop.store(true, Ordering::Relaxed);
                        false
                    } else {
                        true
                    }
                })
            });
        Ok(())
    }
    pub(super) fn live_summaries(&self, node: Uuid) -> Result<Vec<Value>, OperationsError> {
        self.reap_live()?;
        let jobs = self.live.lock().map_err(|_| OperationsError::busy())?;
        let mut values = Vec::new();
        for job in jobs.values() {
            let job = job.lock().map_err(|_| OperationsError::busy())?;
            if job.node == node {
                values.push(job.summary());
            }
        }
        values.sort_unstable_by(|a, b| b["started_at"].as_str().cmp(&a["started_at"].as_str()));
        Ok(values)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Presence {
    viewer: Uuid,
    attached: bool,
}

pub(super) async fn presence(
    State(state): State<OperationsState>,
    Path(node): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
    Json(request): Json<Presence>,
) -> Result<Response, OperationsError> {
    validate_origin(&headers, &uri)?;
    let _ = target(&state, node)?;
    state.disk_space.reap_live()?;
    let jobs = state
        .disk_space
        .live
        .lock()
        .map_err(|_| OperationsError::busy())?;
    let now = Instant::now();
    for job in jobs.values() {
        let mut job = job.lock().map_err(|_| OperationsError::busy())?;
        if job.node == node {
            job.presence(request.viewer, request.attached, now);
        }
    }
    Ok(response(json!({"retention_hours":24})))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn job() -> LiveScan {
        LiveScan::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            &Request {
                path: "/fixture".into(),
                mode: "basic".into(),
                seconds: 0,
                max_entries: 0,
                scope: "directory".into(),
                storage: "realtime".into(),
                viewer: None,
                ..Request::default()
            },
            Arc::new(AtomicBool::new(false)),
        )
    }
    fn entry(id: i64, parent: Option<i64>, size: i64) -> Value {
        json!({"id":id,"parent":parent,"name":if id==0 {"/fixture".into()} else {format!("dir-{id:04}")},
            "kind":"directory","allocated":size,"apparent":size*2,"files":1,"directories":0,
            "issue":null,"incomplete":true,"mtime":0,"scan_path":null})
    }
    #[test]
    fn multiple_viewers_keep_results_and_last_close_starts_full_day() {
        let mut job = job();
        let start = Instant::now();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        job.presence(a, true, start);
        job.presence(b, true, start);
        job.presence(a, false, start + Duration::from_secs(1));
        assert!(job.idle_since.is_none());
        let later = start + RETENTION * 2;
        job.presence(b, true, later);
        assert!(!job.expired(later));
        job.presence(b, false, later);
        assert!(!job.expired(later + RETENTION - Duration::from_secs(1)));
        assert!(job.expired(later + RETENTION));
    }
    #[test]
    fn crashes_expire_after_lease_plus_day_and_reopening_renews() {
        let mut job = job();
        let start = Instant::now();
        let viewer = Uuid::new_v4();
        job.presence(viewer, true, start);
        assert!(!job.expired(start + VIEWER_LEASE + RETENTION - Duration::from_secs(1)));
        job.presence(
            viewer,
            true,
            start + VIEWER_LEASE + RETENTION - Duration::from_secs(1),
        );
        assert!(!job.expired(start + VIEWER_LEASE + RETENTION));
        assert!(job.expired(start + VIEWER_LEASE * 2 + RETENTION * 2));
    }
    #[test]
    fn running_tree_pages_sort_entire_directory_and_preserve_totals() {
        let mut job = job();
        job.metadata(
            &json!({"root":"/fixture","username":"fixture","uid":1000,"scope":"directory"}),
        );
        job.insert(&[entry(0, None, 1_000_000)]).unwrap();
        job.insert(
            &(1..=501)
                .map(|id| entry(id, Some(0), id))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let mut query = EntryQuery {
            parent: 0,
            offset: 250,
            ascending: false,
            search: String::new(),
            basis: "allocated".into(),
            sort: "size".into(),
        };
        let page = job.page(&query).unwrap();
        assert_eq!(page["total"], 501);
        assert_eq!(page["entries"][0]["id"], 251);
        assert_eq!(page["entries"].as_array().unwrap().len(), 250);
        assert_eq!(page["top"].as_array().unwrap().len(), 40);
        assert_eq!(
            page["direct"]["allocated"],
            1_000_000 - (1..=501).sum::<i64>()
        );
        query.search = "dir-0001".into();
        query.offset = 0;
        let filtered = job.page(&query).unwrap();
        assert_eq!(filtered["total"], 1);
        assert_eq!(filtered["direct"], page["direct"]);
        query.parent = 501;
        query.search.clear();
        let nested = job.page(&query).unwrap();
        assert_eq!(nested["ancestors"].as_array().unwrap().len(), 2);
        assert_eq!(job.detail()["result"]["status"], "running");
        assert_eq!(job.detail()["result"]["nodes"].as_array().unwrap().len(), 1);
        job.insert(&[entry(501, Some(0), 9999)]).unwrap();
        assert_eq!(
            job.page(&EntryQuery { parent: 0, ..query }).unwrap()["entries"][0]["id"],
            501
        );
        let mut changed = entry(501, Some(0), 1);
        changed["name"] = json!("changed");
        assert!(job.insert(&[changed]).is_err());
        job.finish(
            "complete",
            Some(json!({"status":"complete","nodes":[entry(0,None,1_000_000)]})),
            "",
        );
        assert_eq!(job.detail()["result"]["status"], "complete");
        assert!(job.summary().get("result").is_none());
    }
    #[test]
    fn memory_jobs_are_multiple_node_scoped_and_reaped_without_three_snapshot_limit() {
        let scans = Scans::default();
        let node = Uuid::new_v4();
        let mut ids = Vec::new();
        for _ in 0..5 {
            let mut job = job();
            job.node = node;
            let id = Uuid::new_v4();
            ids.push(id);
            scans
                .live
                .lock()
                .unwrap()
                .insert(id, Arc::new(Mutex::new(job)));
        }
        assert_eq!(scans.live_summaries(node).unwrap().len(), 5);
        assert!(scans.live_job(Uuid::new_v4(), ids[0]).unwrap().is_none());
        let first = scans.live_job(node, ids[0]).unwrap().unwrap();
        first.lock().unwrap().idle_since = Some(Instant::now() - RETENTION);
        scans.reap_live().unwrap();
        assert_eq!(scans.live_summaries(node).unwrap().len(), 4);
        assert!(first.lock().unwrap().stop.load(Ordering::Relaxed));
    }
}
