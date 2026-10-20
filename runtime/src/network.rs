//! Constant-size counters only: never retain URLs, headers, credentials or bodies.
use chrono::{DateTime, Utc};
use easymanage_protocol::{NetworkActivity, NetworkGroup, NetworkGroupMetric};
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};

#[derive(Default)]
pub struct Counters {
    started: AtomicU64,
    completed: AtomicU64,
    failed: AtomicU64,
    in_flight: AtomicU64,
    connections: AtomicU64,
    received: AtomicU64,
    submitted: AtomicU64,
}
static COUNTERS: OnceLock<[Counters; 14]> = OnceLock::new();
static STARTED: OnceLock<DateTime<Utc>> = OnceLock::new();

fn add(value: &AtomicU64, n: u64) {
    let _ = value.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
        Some(old.saturating_add(n))
    });
}
fn sub(value: &AtomicU64) {
    let _ = value.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
        Some(old.saturating_sub(1))
    });
}
pub fn counter(group: NetworkGroup) -> &'static Counters {
    STARTED.get_or_init(Utc::now);
    &COUNTERS.get_or_init(|| std::array::from_fn(|_| Counters::default()))[group as usize]
}
impl Counters {
    pub fn receive(&self, n: usize) {
        add(&self.received, n as u64);
    }
    pub fn submit(&self, n: usize) {
        add(&self.submitted, n as u64);
    }
    pub fn begin(&self) -> Attempt<'_> {
        add(&self.started, 1);
        add(&self.in_flight, 1);
        Attempt {
            counters: self,
            done: false,
        }
    }
    pub fn connection(&self) -> Connection<'_> {
        add(&self.connections, 1);
        Connection(self)
    }
    fn snapshot(&self, group: NetworkGroup) -> NetworkGroupMetric {
        let get = |v: &AtomicU64| v.load(Ordering::Relaxed);
        NetworkGroupMetric {
            group,
            started: get(&self.started),
            completed: get(&self.completed),
            failed: get(&self.failed),
            in_flight: get(&self.in_flight),
            open_connections: matches!(
                group,
                NetworkGroup::SshConnect | NetworkGroup::SshTunnel | NetworkGroup::WebSocket
            )
            .then(|| get(&self.connections)),
            received_bytes: get(&self.received),
            submitted_bytes: get(&self.submitted),
        }
    }
}
pub struct Attempt<'a> {
    counters: &'a Counters,
    done: bool,
}
impl Attempt<'_> {
    pub fn finish(&mut self, ok: bool) {
        if self.done {
            return;
        }
        self.done = true;
        if !ok {
            add(&self.counters.failed, 1);
        }
        add(&self.counters.completed, 1);
        sub(&self.counters.in_flight);
    }
}
impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        self.finish(false);
    }
}
pub struct Connection<'a>(&'a Counters);
impl Drop for Connection<'_> {
    fn drop(&mut self) {
        sub(&self.0.connections);
    }
}
pub fn snapshot(agent: bool) -> NetworkActivity {
    let started_at = *STARTED.get_or_init(Utc::now);
    NetworkActivity {
        process_id: std::process::id(),
        started_at,
        collected_at: Utc::now(),
        groups: NetworkGroup::ALL
            .into_iter()
            .filter(|g| g.is_agent() == agent)
            .map(|g| counter(g).snapshot(g))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accounts_attempts_cancellation_bytes_and_connection_lifetimes() {
        let counters = Counters::default();
        {
            let mut attempt = counters.begin();
            counters.receive(3);
            counters.submit(5);
            attempt.finish(true);
            attempt.finish(true);
        }
        {
            let _cancelled = counters.begin();
        }
        {
            let _connection = counters.connection();
            assert_eq!(
                counters.snapshot(NetworkGroup::SshConnect).open_connections,
                Some(1)
            );
        }
        let value = counters.snapshot(NetworkGroup::HttpBrowser);
        assert_eq!(
            (
                value.started,
                value.completed,
                value.failed,
                value.in_flight
            ),
            (2, 2, 1, 0)
        );
        assert_eq!(
            (
                value.received_bytes,
                value.submitted_bytes,
                value.open_connections
            ),
            (3, 5, None)
        );
    }
    #[test]
    fn counters_saturate_instead_of_wrapping() {
        let value = AtomicU64::new(u64::MAX);
        add(&value, 1);
        assert_eq!(value.load(Ordering::Relaxed), u64::MAX);
    }
}
