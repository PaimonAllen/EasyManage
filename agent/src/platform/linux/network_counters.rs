//! One bounded procfs read replaces per-interface sysfs/ioctl refreshes.
use easymanage_protocol::NetworkMetric;
use std::{
    collections::BTreeMap,
    io::{self, Read},
    time::Instant,
};
#[derive(Default)]
pub struct Sampler {
    previous: Option<(Instant, BTreeMap<String, (u64, u64)>)>,
}
impl Sampler {
    pub fn sample(&mut self) -> io::Result<Vec<NetworkMetric>> {
        let mut data = String::new();
        std::fs::File::open("/proc/net/dev")?
            .take(1024 * 1024 + 1)
            .read_to_string(&mut data)?;
        if data.len() > 1024 * 1024 {
            return Err(io::Error::other("network counters exceed limit"));
        }
        let current = parse(&data).ok_or_else(|| io::Error::other("invalid network counters"))?;
        Ok(self.update(current, Instant::now()))
    }
    fn update(
        &mut self,
        current: BTreeMap<String, (u64, u64)>,
        now: Instant,
    ) -> Vec<NetworkMetric> {
        let elapsed = self
            .previous
            .as_ref()
            .map(|(at, _)| now.duration_since(*at).as_secs_f64())
            .filter(|seconds| *seconds > 0.0);
        let result = current
            .iter()
            .map(|(name, (received, sent))| {
                let old = self
                    .previous
                    .as_ref()
                    .and_then(|(_, values)| values.get(name));
                let rate = |value: u64, before: Option<u64>| {
                    value
                        .checked_sub(before?)
                        .zip(elapsed)
                        .map(|(bytes, seconds)| bytes as f64 / seconds)
                };
                NetworkMetric {
                    interface: name.clone(),
                    received_bytes_per_second: rate(*received, old.map(|pair| pair.0))
                        .unwrap_or(0.0),
                    transmitted_bytes_per_second: rate(*sent, old.map(|pair| pair.1))
                        .unwrap_or(0.0),
                    total_received_bytes: *received,
                    total_transmitted_bytes: *sent,
                }
            })
            .collect();
        self.previous = Some((now, current));
        result
    }
}
fn parse(data: &str) -> Option<BTreeMap<String, (u64, u64)>> {
    let mut result = BTreeMap::new();
    for line in data.lines().skip(2).filter(|line| !line.trim().is_empty()) {
        let (name, fields) = line.rsplit_once(':')?;
        let name = name.trim();
        if name.is_empty() || name.len() > 64 || result.len() >= 4096 {
            return None;
        }
        let mut fields = fields.split_whitespace();
        let received = fields.next()?.parse().ok()?;
        let sent = fields.nth(7)?.parse().ok()?;
        result.insert(name.to_owned(), (received, sent));
    }
    Some(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rates_use_elapsed_time_and_ignore_removed_interfaces_or_reset_deltas() {
        let mut sampler = Sampler::default();
        let now = Instant::now();
        let fixture = "Inter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets\n eth0: 100 0 0 0 0 0 0 0 200 0 0 0 0 0 0 0\n";
        let first = parse(fixture).unwrap();
        assert_eq!(first["eth0"], (100, 200));
        sampler.update(first, now);
        let second = sampler.update(
            BTreeMap::from([("eth0".into(), (150, 300))]),
            now + std::time::Duration::from_secs(2),
        );
        assert_eq!(second[0].received_bytes_per_second, 25.0);
        assert_eq!(second[0].transmitted_bytes_per_second, 50.0);
        let reset = sampler.update(
            BTreeMap::from([("eth0".into(), (1, 2))]),
            now + std::time::Duration::from_secs(3),
        );
        assert_eq!(reset[0].received_bytes_per_second, 0.0);
        assert!(
            sampler
                .update(BTreeMap::new(), now + std::time::Duration::from_secs(4))
                .is_empty()
        );
        assert!(parse("header\nheader\neth0: invalid").is_none());
    }
}
