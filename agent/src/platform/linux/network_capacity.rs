//! Hourly network capacity probes. Never perform a remote filesystem syscall in
//! the daemon, and never queue another probe behind a stuck child.
use crate::store::Store;
use chrono::{DateTime, Utc};
use easymanage_protocol::{DiskMetric, NetworkCapacityProbe};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, process::Command, task::JoinHandle};

const INTERVAL: i64 = 3600;
const TIMEOUT: Duration = Duration::from_secs(10);
const SCRIPT: &str = include_str!("network_capacity_probe.py");

#[derive(Clone, Default, Deserialize, Serialize)]
struct Cached {
    attempted_at: Option<DateTime<Utc>>,
    collected_at: Option<DateTime<Utc>>,
    total_bytes: u64,
    available_bytes: u64,
    status: String,
}

#[derive(Deserialize)]
struct Outcome {
    status: String,
    #[serde(default)]
    total_bytes: u64,
    #[serde(default)]
    available_bytes: u64,
    #[serde(default)]
    collected_at: Option<DateTime<Utc>>,
}

impl Outcome {
    fn failed(status: &str) -> Self {
        Self {
            status: status.into(),
            total_bytes: 0,
            available_bytes: 0,
            collected_at: None,
        }
    }
}

struct Running {
    key: String,
    started: Instant,
    task: JoinHandle<Outcome>,
}

pub(super) struct Sampler {
    uid: u32,
    lock: std::path::PathBuf,
    cache: BTreeMap<String, Cached>,
    active: Option<Running>,
}

// Multiple bind mounts of one filesystem share capacity, not separate probes.
fn key(disk: &DiskMetric) -> Option<String> {
    let mount = disk.mount.as_ref().filter(|mount| mount.network)?;
    serde_json::to_string(&(&mount.device_id, &disk.file_system, &mount.source)).ok()
}

fn due(cached: &Cached, now: DateTime<Utc>) -> bool {
    cached
        .attempted_at
        .is_none_or(|at| (now - at).num_seconds() >= INTERVAL)
}

impl Sampler {
    pub async fn load(state: &Path, uid: u32, store: &Store) -> Self {
        let cache = store
            .network_capacity_cache()
            .await
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str::<(u32, BTreeMap<String, Cached>)>(&text).ok())
            .filter(|(old_uid, _)| *old_uid == uid)
            .map(|(_, mut cache)| {
                for value in cache.values_mut() {
                    if value.status == "collecting" {
                        value.status = "interrupted".into();
                    }
                }
                cache
            })
            .unwrap_or_default();
        Self {
            uid,
            lock: state.with_file_name(format!("network-capacity-{uid}.lock")),
            cache,
            active: None,
        }
    }

    async fn save(&self, store: &Store) -> bool {
        let Ok(value) = serde_json::to_string(&(self.uid, &self.cache)) else {
            return false;
        };
        if store.save_network_capacity_cache(&value).await.is_err() {
            tracing::warn!("network capacity cache unavailable; postponing new probes");
            return false;
        }
        true
    }

    pub async fn refresh(&mut self, disks: &mut [DiskMetric], store: &Store) {
        let now = Utc::now();
        // Retain recent retry deadlines across temporary inventory failures.
        // A different identity never receives the old filesystem's capacity.
        let keys: std::collections::BTreeSet<_> = disks.iter().filter_map(key).collect();
        self.cache.retain(|key, cached| {
            keys.contains(key)
                || cached
                    .attempted_at
                    .is_some_and(|at| (now - at).num_seconds() < 86400)
        });
        if self
            .active
            .as_ref()
            .is_some_and(|running| running.task.is_finished())
        {
            let running = self.active.take().expect("completed probe");
            let outcome = running
                .task
                .await
                .unwrap_or_else(|_| Outcome::failed("error"));
            if let Some(cached) = self.cache.get_mut(&running.key) {
                if outcome.status == "ok"
                    && outcome.collected_at.is_some()
                    && outcome.total_bytes > 0
                    && outcome.available_bytes <= outcome.total_bytes
                {
                    cached.total_bytes = outcome.total_bytes;
                    cached.available_bytes = outcome.available_bytes;
                    cached.collected_at = outcome.collected_at;
                    cached.status = "ok".into();
                } else {
                    cached.status = if outcome.status == "ok" {
                        "error".into()
                    } else {
                        outcome.status
                    };
                }
                self.save(store).await;
            }
        }
        if let Some(running) = &self.active {
            if running.started.elapsed() >= TIMEOUT {
                if let Some(cached) = self.cache.get_mut(&running.key) {
                    if cached.status != "timeout" {
                        cached.status = "timeout".into();
                        self.save(store).await;
                    }
                }
            }
        } else if let Some(disk) = disks.iter().find(|disk| {
            key(disk).is_some_and(|key| self.cache.get(&key).is_none_or(|cached| due(cached, now)))
        }) {
            let key = key(disk).expect("network identity");
            let cached = self.cache.entry(key.clone()).or_default();
            cached.attempted_at = Some(now);
            cached.status = "collecting".into();
            // Persist the attempt before spawning. Restart/re-enable cannot
            // turn the hourly policy into a burst against an unavailable share.
            if self.save(store).await {
                let mount = disk.mount.as_ref().expect("network mount");
                let request = serde_json::json!({"lock":self.lock,"path":disk.mount_point,
                    "device":mount.device_id,"root":mount.root,"filesystem":disk.file_system,"source":mount.source});
                self.active = Some(Running {
                    key,
                    started: Instant::now(),
                    task: tokio::spawn(probe(request, TIMEOUT)),
                });
            } else if let Some(cached) = self.cache.get_mut(&key) {
                cached.status = "error".into();
            }
        }
        for disk in disks {
            let Some(key) = key(disk) else {
                continue;
            };
            let cached = self.cache.get(&key).cloned().unwrap_or_default();
            let mount = disk.mount.as_mut().expect("network mount");
            mount.capacity_known = cached.collected_at.is_some()
                && cached.total_bytes > 0
                && cached.available_bytes <= cached.total_bytes;
            if mount.capacity_known {
                disk.total_bytes = cached.total_bytes;
                disk.available_bytes = cached.available_bytes;
            }
            mount.capacity_probe = Some(NetworkCapacityProbe {
                interval_seconds: INTERVAL as u64,
                attempted_at: cached.attempted_at,
                collected_at: cached.collected_at,
                status: if cached.status.is_empty() {
                    "pending".into()
                } else {
                    cached.status
                },
            });
        }
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        if let Some(active) = &self.active {
            active.task.abort();
        }
    }
}

async fn probe(request: serde_json::Value, timeout: Duration) -> Outcome {
    let mut command = Command::new("/usr/bin/python3");
    command
        .args(["-I", "-S", "-c", SCRIPT, &request.to_string()])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    run_probe(command, timeout).await
}

async fn run_probe(mut command: Command, timeout: Duration) -> Outcome {
    let Ok(mut child) = command.spawn() else {
        return Outcome::failed("error");
    };
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) if status.success() => {
            let Some(stdout) = child.stdout.take() else {
                return Outcome::failed("error");
            };
            let mut bytes = Vec::new();
            if stdout.take(1025).read_to_end(&mut bytes).await.is_err() || bytes.len() > 1024 {
                return Outcome::failed("error");
            }
            serde_json::from_slice(&bytes).unwrap_or_else(|_| Outcome::failed("error"))
        }
        Err(_) => {
            let _ = child.start_kill();
            // A hard NFS mount can keep a killed process in D state. Retain the
            // handle and lock; do not spawn replacements while it remains alive.
            let _ = child.wait().await;
            Outcome::failed("timeout")
        }
        _ => Outcome::failed("error"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use easymanage_protocol::MountIdentity;

    #[tokio::test]
    async fn real_child_timeout_kills_and_reaps_without_waiting_for_the_query() {
        let mut command = Command::new("/usr/bin/python3");
        command
            .args(["-I", "-S", "-c", "import time; time.sleep(60)"])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let outcome = tokio::time::timeout(
            Duration::from_secs(2),
            run_probe(command, Duration::from_millis(30)),
        )
        .await
        .unwrap();
        assert_eq!(outcome.status, "timeout");
    }

    fn disk(path: &str) -> DiskMetric {
        DiskMetric {
            name: "198.51.100.1:/fixture".into(),
            mount_point: path.into(),
            file_system: "nfs4".into(),
            total_bytes: 0,
            available_bytes: 0,
            read_bytes_per_second: None,
            written_bytes_per_second: None,
            mount: Some(MountIdentity {
                device_id: "0:41".into(),
                root: "/".into(),
                source: "198.51.100.1:/fixture".into(),
                network: true,
                read_only: false,
                capacity_known: false,
                capacity_probe: None,
            }),
        }
    }

    #[tokio::test]
    async fn stalled_probe_keeps_old_capacity_without_blocking_or_spawning_duplicates() {
        let root =
            std::env::temp_dir().join(format!("easymanage-network-{}", uuid::Uuid::new_v4()));
        let state = root.join("agent.db");
        let store = Store::open(&state).await.unwrap();
        let mut sampler = Sampler::load(&state, 1000, &store).await;
        let mut disks = vec![disk("/nfs"), disk("/bind-alias")];
        let key = key(&disks[0]).unwrap();
        let now = Utc::now();
        sampler.cache.insert(
            key.clone(),
            Cached {
                attempted_at: Some(now),
                collected_at: Some(now - chrono::Duration::hours(1)),
                total_bytes: 1000,
                available_bytes: 300,
                status: "collecting".into(),
            },
        );
        let task = tokio::spawn(std::future::pending());
        let task_id = task.id();
        sampler.active = Some(Running {
            key: key.clone(),
            started: Instant::now() - TIMEOUT,
            task,
        });
        for _ in 0..3 {
            tokio::time::timeout(
                Duration::from_millis(500),
                sampler.refresh(&mut disks, &store),
            )
            .await
            .unwrap();
            assert_eq!(sampler.active.as_ref().unwrap().task.id(), task_id);
            for disk in &disks {
                assert_eq!((disk.total_bytes, disk.available_bytes), (1000, 300));
                assert_eq!(
                    disk.mount
                        .as_ref()
                        .unwrap()
                        .capacity_probe
                        .as_ref()
                        .unwrap()
                        .status,
                    "timeout"
                );
            }
        }
        let abort = sampler.active.as_ref().unwrap().task.abort_handle();
        drop(sampler);
        tokio::task::yield_now().await;
        assert!(abort.is_finished());
        let mut sampler = Sampler::load(&state, 1000, &store).await;
        sampler.refresh(&mut disks, &store).await;
        assert!(
            sampler.active.is_none(),
            "restart must respect persisted hourly attempt"
        );
        assert_eq!(sampler.cache.len(), 1, "bind mounts share one capacity");
        assert!(!due(
            sampler.cache.get(&key).unwrap(),
            now + chrono::Duration::seconds(3599)
        ));
        assert!(due(
            sampler.cache.get(&key).unwrap(),
            now + chrono::Duration::seconds(3600)
        ));
        // Temporary missing inventory cannot erase the retry deadline.
        sampler.refresh(&mut [], &store).await;
        sampler.refresh(&mut disks, &store).await;
        assert!(sampler.active.is_none());
        assert!(
            Sampler::load(&state, 0, &store).await.cache.is_empty(),
            "privilege changes invalidate cached observations"
        );
        assert_ne!(
            key,
            super::key(&DiskMetric {
                mount: Some(MountIdentity {
                    device_id: "0:99".into(),
                    ..disks[0].mount.clone().unwrap()
                }),
                ..disks[0].clone()
            })
            .unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn completed_sample_keeps_actual_timestamp_and_cached_success_on_error() {
        let root =
            std::env::temp_dir().join(format!("easymanage-network-{}", uuid::Uuid::new_v4()));
        let state = root.join("agent.db");
        let store = Store::open(&state).await.unwrap();
        let mut sampler = Sampler::load(&state, 1000, &store).await;
        let mut disks = vec![disk("/nfs")];
        let key = key(&disks[0]).unwrap();
        let at = Utc::now() - chrono::Duration::seconds(5);
        sampler.cache.insert(
            key.clone(),
            Cached {
                attempted_at: Some(at),
                ..Default::default()
            },
        );
        for outcome in [
            Outcome {
                status: "ok".into(),
                total_bytes: 500,
                available_bytes: 100,
                collected_at: Some(at),
            },
            Outcome::failed("error"),
        ] {
            let task = tokio::spawn(async move { outcome });
            while !task.is_finished() {
                tokio::task::yield_now().await;
            }
            sampler.active = Some(Running {
                key: key.clone(),
                started: Instant::now(),
                task,
            });
            sampler.refresh(&mut disks, &store).await;
            let mount = disks[0].mount.as_ref().unwrap();
            assert!(mount.capacity_known);
            assert_eq!(disks[0].available_bytes, 100);
            assert_eq!(
                mount.capacity_probe.as_ref().unwrap().collected_at,
                Some(at)
            );
            assert!(sampler.active.is_none());
        }
        assert_eq!(
            disks[0]
                .mount
                .as_ref()
                .unwrap()
                .capacity_probe
                .as_ref()
                .unwrap()
                .status,
            "error"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
