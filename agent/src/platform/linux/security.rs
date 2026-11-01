//! Fixed hourly, UID-scoped security snapshots, independent of inventory refresh.
use crate::store::Store;
use chrono::{DateTime, Utc};
use easymanage_protocol::HostSecurity;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

pub const INTERVAL: Duration = Duration::from_secs(3600);

pub struct Sampler {
    uid: u32,
    cached: Option<HostSecurity>,
    refreshed_at: Option<Instant>,
}
pub fn cache_age(snapshot: &HostSecurity, now: DateTime<Utc>) -> Option<Duration> {
    if snapshot.interval_seconds != Some(INTERVAL.as_secs()) {
        return None;
    }
    let age = now
        .signed_duration_since(snapshot.collected_at?)
        .to_std()
        .ok()?;
    (age < INTERVAL).then_some(age)
}
impl Sampler {
    pub async fn load(store: &Store, uid: u32) -> Self {
        let snapshot = store.host_security(uid).await.ok().flatten();
        let age = snapshot
            .as_ref()
            .and_then(|sample| cache_age(sample, Utc::now()));
        Self {
            uid,
            cached: if age.is_some() { snapshot } else { None },
            refreshed_at: age.and_then(|age| Instant::now().checked_sub(age)),
        }
    }
    pub async fn sample(&mut self, store: &Store) -> HostSecurity {
        self.sample_from(store, Path::new("/"), Instant::now())
            .await
    }
    async fn sample_from(&mut self, store: &Store, root: &Path, now: Instant) -> HostSecurity {
        if self.cached.is_none()
            || self
                .refreshed_at
                .is_none_or(|at| now.saturating_duration_since(at) >= INTERVAL)
        {
            let snapshot = probe(root);
            if let Err(error) = store.save_host_security(self.uid, &snapshot).await {
                tracing::warn!(%error,"hourly security snapshot cache could not be persisted");
            }
            self.cached = Some(snapshot);
            self.refreshed_at = Some(now);
        }
        self.cached
            .as_ref()
            .expect("security snapshot initialized")
            .clone()
    }
}
fn read(root: &Path, path: &str, limit: u64) -> Option<String> {
    let mut value = String::new();
    std::fs::File::open(root.join(path))
        .ok()?
        .take(limit + 1)
        .read_to_string(&mut value)
        .ok()?;
    (value.len() as u64 <= limit).then_some(value)
}
fn probe(root: &Path) -> HostSecurity {
    HostSecurity {
        collected_at: Some(Utc::now()),
        interval_seconds: Some(INTERVAL.as_secs()),
        apparmor_enabled: read(root, "sys/module/apparmor/parameters/enabled", 64).and_then(
            |value| match value.trim() {
                "Y" => Some(true),
                "N" => Some(false),
                _ => None,
            },
        ),
        aslr_mode: read(root, "proc/sys/kernel/randomize_va_space", 64)
            .and_then(|value| value.trim().parse::<u8>().ok())
            .filter(|value| *value <= 2),
        ptrace_scope: read(root, "proc/sys/kernel/yama/ptrace_scope", 64)
            .and_then(|value| value.trim().parse::<u8>().ok())
            .filter(|value| *value <= 3),
        kernel_lockdown: read(root, "sys/kernel/security/lockdown", 128).and_then(|value| {
            value
                .split_whitespace()
                .find_map(|part| part.strip_prefix('[')?.strip_suffix(']'))
                .filter(|mode| matches!(*mode, "none" | "integrity" | "confidentiality"))
                .map(str::to_owned)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn security_probes_are_hourly_persisted_and_scoped_to_effective_uid() {
        let root =
            std::env::temp_dir().join(format!("easymanage-security-{}", uuid::Uuid::new_v4()));
        let path = root.join("sys/module/apparmor/parameters/enabled");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "Y").unwrap();
        let store = Store::open(&root.join("state.db")).await.unwrap();
        let mut sampler = Sampler::load(&store, 1001).await;
        let now = Instant::now();
        let first = sampler.sample_from(&store, &root, now).await;
        assert_eq!(first.apparmor_enabled, Some(true));
        assert_eq!(first.aslr_mode, None);
        std::fs::write(&path, "N").unwrap();
        let cached = sampler
            .sample_from(&store, &root, now + Duration::from_secs(3599))
            .await;
        assert_eq!(cached.collected_at, first.collected_at);
        assert_eq!(cached.apparmor_enabled, Some(true));
        drop(sampler);
        drop(store);
        let reopened = Store::open(&root.join("state.db")).await.unwrap();
        let mut restored = Sampler::load(&reopened, 1001).await;
        let restored_sample = restored.sample_from(&reopened, &root, Instant::now()).await;
        assert_eq!(restored_sample.collected_at, first.collected_at);
        assert_eq!(restored_sample.apparmor_enabled, Some(true));
        let updated = restored
            .sample_from(&reopened, &root, Instant::now() + INTERVAL)
            .await;
        assert_eq!(updated.apparmor_enabled, Some(false));
        let other_uid = Sampler::load(&reopened, 0).await;
        assert!(other_uid.cached.is_none());
        assert!(
            cache_age(
                &first,
                first.collected_at.unwrap() + chrono::Duration::hours(1)
            )
            .is_none()
        );
        assert!(
            cache_age(
                &first,
                first.collected_at.unwrap() - chrono::Duration::seconds(1)
            )
            .is_none()
        );
        drop(restored);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }
}
