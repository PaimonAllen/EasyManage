//! Short-lived, bounded, single-flight caches for public monitoring views only.
//! Mutations advance the epoch after commit, including while a read is in flight.
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

static EPOCH: AtomicU64 = AtomicU64::new(0);
pub const TTL: Duration = Duration::from_secs(1);
const MAX_ENTRY_BYTES: usize = 8 * 1024 * 1024;

pub fn invalidate() {
    EPOCH.fetch_add(1, Ordering::SeqCst);
}

struct Entry<T> {
    epoch: u64,
    at: Instant,
    value: T,
}

pub struct SnapshotCache<T> {
    entry: Mutex<Option<Entry<T>>>,
    epoch: &'static AtomicU64,
}

impl<T: Clone> SnapshotCache<T> {
    pub const fn new() -> Self {
        Self::with_epoch(&EPOCH)
    }

    const fn with_epoch(epoch: &'static AtomicU64) -> Self {
        Self {
            entry: Mutex::const_new(None),
            epoch,
        }
    }

    pub async fn get<E>(
        &self,
        load: impl std::future::Future<Output = Result<T, E>>,
        bytes: impl FnOnce(&T) -> usize,
    ) -> Result<T, E> {
        // Concurrent viewers share one load, rather than all missing together.
        // A cancelled/failed load cannot populate the cache.
        let mut entry = self.entry.lock().await;
        let epoch = self.epoch.load(Ordering::SeqCst);
        if let Some(cached) = entry.as_ref()
            && cached.epoch == epoch
            && cached.at.elapsed() < TTL
        {
            return Ok(cached.value.clone());
        }
        *entry = None;
        let value = load.await?;
        if bytes(&value) <= MAX_ENTRY_BYTES && self.epoch.load(Ordering::SeqCst) == epoch {
            *entry = Some(Entry {
                epoch,
                at: Instant::now(),
                value: value.clone(),
            });
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicUsize};

    #[tokio::test]
    async fn concurrent_viewers_share_one_load_and_expired_entries_refresh() {
        static EPOCH: AtomicU64 = AtomicU64::new(0);
        let cache = Arc::new(SnapshotCache::with_epoch(&EPOCH));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..24 {
            let (cache, calls) = (cache.clone(), calls.clone());
            tasks.spawn(async move {
                cache
                    .get(
                        async {
                            calls.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(5)).await;
                            Ok::<_, ()>("same snapshot")
                        },
                        |value| value.len(),
                    )
                    .await
            });
        }
        while let Some(result) = tasks.join_next().await {
            assert_eq!(result.unwrap().unwrap(), "same snapshot");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        cache.entry.lock().await.as_mut().unwrap().at = Instant::now() - TTL;
        assert_eq!(
            cache
                .get(async { Ok::<_, ()>("new snapshot") }, |value| value.len())
                .await
                .unwrap(),
            "new snapshot"
        );
    }

    #[tokio::test]
    async fn failed_oversized_and_cancelled_loads_are_not_retained() {
        static EPOCH: AtomicU64 = AtomicU64::new(0);
        let cache = SnapshotCache::with_epoch(&EPOCH);
        assert!(
            cache
                .get(async { Err::<&str, _>(()) }, |_| 0)
                .await
                .is_err()
        );
        assert!(cache.entry.lock().await.is_none());
        assert_eq!(
            cache
                .get(async { Ok::<_, ()>("too big") }, |_| MAX_ENTRY_BYTES + 1)
                .await
                .unwrap(),
            "too big"
        );
        assert!(cache.entry.lock().await.is_none());
        assert!(
            tokio::time::timeout(
                Duration::from_millis(5),
                cache.get(std::future::pending::<Result<&str, ()>>(), |_| 0)
            )
            .await
            .is_err()
        );
        assert_eq!(
            cache
                .get(async { Ok::<_, ()>("recovered") }, |value| value.len())
                .await
                .unwrap(),
            "recovered"
        );
    }

    #[tokio::test]
    async fn a_write_during_read_cannot_repopulate_an_invalidated_snapshot() {
        static EPOCH: AtomicU64 = AtomicU64::new(0);
        let cache = SnapshotCache::with_epoch(&EPOCH);
        let value = cache
            .get(
                async {
                    EPOCH.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, ()>("read before commit")
                },
                |value| value.len(),
            )
            .await
            .unwrap();
        assert_eq!(value, "read before commit");
        assert!(cache.entry.lock().await.is_none());
        cache
            .get(async { Ok::<_, ()>("after commit") }, |value| value.len())
            .await
            .unwrap();
        EPOCH.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            cache
                .get(async { Ok::<_, ()>("latest") }, |value| value.len())
                .await
                .unwrap(),
            "latest"
        );
    }
}
