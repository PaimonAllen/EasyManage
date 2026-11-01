//! One bounded procfs read per 30 seconds; no commands or per-CPU probes.
use chrono::Utc;
use easymanage_protocol::{CpuFrequency, CpuSocketMetric};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

const INTERVAL: Duration = Duration::from_secs(30);
const MAX_BYTES: u64 = 2 * 1024 * 1024;
const MAX_CPUS: u32 = 4096;

#[derive(Default)]
pub struct Sampler {
    refreshed_at: Option<Instant>,
    cached: Option<CpuFrequency>,
    sockets: Vec<Socket>,
}
struct Socket {
    id: u32,
    model: String,
    members: Vec<u32>,
    cores: Option<u32>,
    frequency: Option<CpuFrequency>,
}
impl Sampler {
    pub fn sample(&mut self) -> Option<CpuFrequency> {
        self.sample_from(Path::new("/proc/cpuinfo"), Instant::now())
    }
    fn sample_from(&mut self, path: &Path, now: Instant) -> Option<CpuFrequency> {
        if self
            .refreshed_at
            .is_none_or(|at| now.saturating_duration_since(at) >= INTERVAL)
        {
            match read(path) {
                Some((frequency, sockets)) => {
                    self.cached = frequency;
                    self.sockets = sockets;
                }
                None => {
                    self.cached = None;
                    self.sockets.clear();
                }
            }
            // Missing/unsupported data gets the same backoff as a valid read.
            self.refreshed_at = Some(now);
        }
        self.cached.clone()
    }
    pub fn socket_metrics(
        &self,
        cpus: &[sysinfo::Cpu],
        temperatures: &BTreeMap<u32, f32>,
    ) -> Vec<CpuSocketMetric> {
        let usage: HashMap<u32, f32> = cpus
            .iter()
            .filter_map(|cpu| {
                Some((
                    cpu.name().strip_prefix("cpu")?.parse().ok()?,
                    cpu.cpu_usage(),
                ))
            })
            .filter(|(_, value)| value.is_finite() && (0.0..=100.0).contains(value))
            .collect();
        self.socket_metrics_from_usage(&usage, temperatures)
    }
    fn socket_metrics_from_usage(
        &self,
        usage: &HashMap<u32, f32>,
        temperatures: &BTreeMap<u32, f32>,
    ) -> Vec<CpuSocketMetric> {
        self.sockets
            .iter()
            .map(|socket| {
                let values: Option<Vec<f32>> = socket
                    .members
                    .iter()
                    .map(|id| usage.get(id).copied())
                    .collect();
                CpuSocketMetric {
                    id: socket.id,
                    model: socket.model.clone(),
                    logical_cpus: socket.members.len() as u32,
                    physical_cores: socket.cores,
                    usage_percent: values
                        .filter(|values| !values.is_empty())
                        .map(|values| values.iter().sum::<f32>() / values.len() as f32),
                    frequency: socket.frequency.clone(),
                    temperature_celsius: temperatures.get(&socket.id).copied(),
                }
            })
            .collect()
    }
}

fn read(path: &Path) -> Option<(Option<CpuFrequency>, Vec<Socket>)> {
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() as u64 > MAX_BYTES {
        return None;
    }
    Some((parse(&text), parse_sockets(&text)))
}

fn parse_sockets(text: &str) -> Vec<Socket> {
    struct Group {
        model: String,
        members: Vec<u32>,
        cores: Option<u32>,
        frequency: String,
    }
    let mut groups: BTreeMap<u32, Group> = BTreeMap::new();
    let mut seen = HashSet::new();
    for block in text.split("\n\n") {
        let fields: HashMap<&str, &str> = block
            .lines()
            .filter_map(|line| line.split_once(':'))
            .map(|(k, v)| (k.trim(), v.trim()))
            .collect();
        let Some(cpu) = fields
            .get("processor")
            .and_then(|id| id.parse::<u32>().ok())
        else {
            continue;
        };
        // Missing package IDs are unknown topology, never one CPU per thread.
        let Some(id) = fields
            .get("physical id")
            .and_then(|id| id.parse::<u32>().ok())
        else {
            return Vec::new();
        };
        if !seen.insert(cpu) || seen.len() > MAX_CPUS as usize {
            return Vec::new();
        }
        let cores = fields
            .get("cpu cores")
            .and_then(|n| n.parse::<u32>().ok())
            .filter(|n| *n > 0);
        let group = groups.entry(id).or_insert_with(|| Group {
            model: fields
                .get("model name")
                .unwrap_or(&"")
                .chars()
                .take(256)
                .collect(),
            members: Vec::new(),
            cores,
            frequency: String::new(),
        });
        if group.cores != cores {
            group.cores = None;
        }
        group.members.push(cpu);
        if let Some(value) = fields.get("cpu MHz").or_else(|| fields.get("CPU MHz")) {
            group.frequency.push_str("cpu MHz:");
            group.frequency.push_str(value);
            group.frequency.push('\n');
        }
        if groups.len() > 64 {
            return Vec::new();
        }
    }
    groups
        .into_iter()
        .map(|(id, g)| Socket {
            id,
            model: g.model,
            cores: g.cores.filter(|cores| *cores <= g.members.len() as u32),
            members: g.members,
            frequency: parse(&g.frequency),
        })
        .collect()
}
fn parse(text: &str) -> Option<CpuFrequency> {
    let mut count = 0_u32;
    let mut sum = 0.0_f64;
    let mut minimum = f64::INFINITY;
    let mut maximum = 0.0_f64;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if !matches!(key.trim(), "cpu MHz" | "CPU MHz") {
            continue;
        }
        let Some(value) = value
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite() && *value > 0.0 && *value <= 20_000.0)
        else {
            continue;
        };
        count += 1;
        if count > MAX_CPUS {
            return None;
        }
        sum += value;
        minimum = minimum.min(value);
        maximum = maximum.max(value);
    }
    (count > 0).then(|| CpuFrequency {
        collected_at: Utc::now(),
        average_mhz: sum / f64::from(count),
        minimum_mhz: minimum,
        maximum_mhz: maximum,
        logical_cpus: count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frequencies_include_idle_cores_and_ignore_bogomips_and_invalid_readings() {
        let sample=parse("cpu MHz : 800\ncpu MHz : 4200\nBogoMIPS : 9900\ncpu MHz : NaN\ncpu MHz : 0\ncpu MHz : -2\nCPU MHz : 2500\ncpu MHz : 20001\n").unwrap();
        assert_eq!(sample.logical_cpus, 3);
        assert_eq!(sample.average_mhz, 2500.0);
        assert_eq!(sample.minimum_mhz, 800.0);
        assert_eq!(sample.maximum_mhz, 4200.0);
        assert!(parse("BogoMIPS: 12000\ncpu MHz: N/A\ncpu MHz: 0").is_none());
    }
    #[test]
    fn repeated_heartbeats_reuse_the_sample_and_missing_data_is_not_polled_or_zero() {
        let root =
            std::env::temp_dir().join(format!("easymanage-cpu-frequency-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("cpuinfo");
        std::fs::write(&path, "cpu MHz: 2800").unwrap();
        let now = Instant::now();
        let mut sampler = Sampler::default();
        let first = sampler.sample_from(&path, now).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            sampler
                .sample_from(&path, now + Duration::from_secs(29))
                .unwrap()
                .collected_at,
            first.collected_at
        );
        assert!(sampler.sample_from(&path, now + INTERVAL).is_none());
        std::fs::write(&path, "cpu MHz: 3200").unwrap();
        assert!(
            sampler
                .sample_from(&path, now + Duration::from_secs(59))
                .is_none()
        );
        assert_eq!(
            sampler
                .sample_from(&path, now + INTERVAL * 2)
                .unwrap()
                .average_mhz,
            3200.0
        );
        std::fs::write(&path, "a".repeat(MAX_BYTES as usize + 1)).unwrap();
        assert!(sampler.sample_from(&path, now + INTERVAL * 3).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn physical_packages_group_smt_threads_and_keep_noncontiguous_cpu_ids_independent() {
        let block = |cpu, socket, mhz| {
            format!(
                "processor: {cpu}\nphysical id: {socket}\nmodel name: Socket {socket}\ncpu cores: 1\ncpu MHz: {mhz}\n\n"
            )
        };
        let text =
            block(0, 2, 800) + &block(4, 2, 3200) + &block(10, 7, 4000) + &block(20, 7, 2000);
        let sampler = Sampler {
            sockets: parse_sockets(&text),
            ..Sampler::default()
        };
        let sockets = sampler.socket_metrics_from_usage(
            &HashMap::from([(0, 10.0), (4, 30.0), (10, 60.0), (20, 80.0)]),
            &BTreeMap::new(),
        );
        assert_eq!(sockets.len(), 2);
        assert_eq!(
            (
                sockets[0].id,
                sockets[0].logical_cpus,
                sockets[0].physical_cores
            ),
            (2, 2, Some(1))
        );
        assert_eq!(sockets[0].usage_percent, Some(20.0));
        assert_eq!(sockets[1].usage_percent, Some(70.0));
        assert_eq!(sockets[0].frequency.as_ref().unwrap().average_mhz, 2000.0);
        assert_eq!(sockets[1].frequency.as_ref().unwrap().average_mhz, 3000.0);
        assert!(sockets[0].temperature_celsius.is_none());
        assert!(
            sampler.socket_metrics_from_usage(&HashMap::from([(0, 10.0)]), &BTreeMap::new())[0]
                .usage_percent
                .is_none()
        );
        let temperatures =
            sampler.socket_metrics_from_usage(&HashMap::new(), &BTreeMap::from([(7, 62.0)]));
        assert_eq!(temperatures[0].temperature_celsius, None);
        assert_eq!(temperatures[1].temperature_celsius, Some(62.0));
        assert!(
            parse_sockets("processor:0\ncpu MHz:2800\n\nprocessor:1\ncpu MHz:2800\n").is_empty()
        );
        assert!(parse_sockets(&(block(0, 2, 800) + &block(0, 7, 3200))).is_empty());
    }
}
