//! Package database metadata invalidates this hourly cache after real installs.
use crate::platform::common::command::read_command;
use std::{
    io,
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::Mutex;
struct Cached {
    at: Instant,
    signature: Option<(SystemTime, u64)>,
    value: Result<String, io::ErrorKind>,
}
static CACHE: Mutex<Option<Cached>> = Mutex::const_new(None);
impl Cached {
    fn fresh(&self, signature: Option<(SystemTime, u64)>, now: Instant) -> bool {
        let interval = if self.value.is_ok() {
            Duration::from_secs(3600)
        } else {
            Duration::from_secs(60)
        };
        self.signature == signature && now.saturating_duration_since(self.at) < interval
    }
}
pub async fn collect() -> io::Result<String> {
    let signature = std::fs::metadata("/var/lib/dpkg/status")
        .ok()
        .and_then(|meta| Some((meta.modified().ok()?, meta.len())));
    let mut cache = CACHE.lock().await;
    if let Some(cached) = cache.as_ref() {
        if cached.fresh(signature, Instant::now()) {
            return cached
                .value
                .clone()
                .map_err(|kind| io::Error::new(kind, "package inventory unavailable"));
        }
    }
    let result = read_command(
        "dpkg-query",
        &["-W", "-f=${Package}\\t${Version}\\n"],
        1024 * 1024,
    )
    .await;
    *cache = Some(Cached {
        at: Instant::now(),
        signature,
        value: result.as_ref().map(Clone::clone).map_err(io::Error::kind),
    });
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_refreshes_after_installs_or_expiry_and_retries_failures_earlier() {
        let now = Instant::now();
        let signature = Some((SystemTime::UNIX_EPOCH, 100));
        let cached = Cached {
            at: now,
            signature,
            value: Ok("packages".into()),
        };
        assert!(cached.fresh(signature, now + Duration::from_secs(3599)));
        assert!(!cached.fresh(signature, now + Duration::from_secs(3600)));
        assert!(!cached.fresh(Some((SystemTime::UNIX_EPOCH, 101)), now));
        let failure = Cached {
            value: Err(io::ErrorKind::NotFound),
            ..cached
        };
        assert!(failure.fresh(signature, now + Duration::from_secs(59)));
        assert!(!failure.fresh(signature, now + Duration::from_secs(60)));
    }
}
