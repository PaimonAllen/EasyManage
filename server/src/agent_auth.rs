//! Bounded cache for the one-second live lane only. Durable/control reports
//! still verify PostgreSQL. Enrollment and soft-delete revoke cache entries.
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;
const TTL: Duration = Duration::from_secs(10);
#[derive(Default)]
struct Cache {
    epoch: u64,
    entries: HashMap<Uuid, (String, Instant)>,
}
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Mutex::default);
pub fn generation() -> Option<u64> {
    CACHE.lock().ok().map(|cache| cache.epoch)
}
pub fn accepts(id: Uuid, hash: &str) -> bool {
    CACHE.lock().ok().is_some_and(|cache| {
        cache.entries.get(&id).is_some_and(|(expected, at)| {
            at.elapsed() < TTL && crate::api::constant_time_equal(expected, hash)
        })
    })
}
pub fn remember(id: Uuid, hash: String, epoch: Option<u64>) {
    if let Ok(mut cache) = CACHE.lock() {
        if Some(cache.epoch) != epoch {
            return;
        }
        if cache.entries.len() >= 1024 {
            cache.entries.retain(|_, (_, at)| at.elapsed() < TTL);
            if cache.entries.len() >= 1024 {
                cache.entries.clear();
            }
        }
        cache.entries.insert(id, (hash, Instant::now()));
    }
}
pub fn invalidate() {
    crate::read_cache::invalidate();
    if let Ok(mut cache) = CACHE.lock() {
        cache.epoch = cache.epoch.wrapping_add(1);
        cache.entries.clear();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revoked_or_rotated_credentials_never_survive_in_flight_cache_fills() {
        invalidate();
        let id = Uuid::new_v4();
        let epoch = generation();
        remember(id, "old-hash".into(), epoch);
        assert!(accepts(id, "old-hash"));
        assert!(!accepts(id, "wrong"));
        invalidate();
        remember(id, "old-hash".into(), epoch);
        assert!(!accepts(id, "old-hash"));
        remember(id, "new-hash".into(), generation());
        assert!(accepts(id, "new-hash"));
        assert!(!accepts(id, "old-hash"));
        CACHE.lock().unwrap().entries.get_mut(&id).unwrap().1 = Instant::now() - TTL;
        assert!(!accepts(id, "new-hash"));
        invalidate();
    }
}
