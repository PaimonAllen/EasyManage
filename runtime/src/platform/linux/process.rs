//! Linux bounded, read-only accounting for the current EasyManage process.
//! No process enumeration or privileged tracing; callers choose the sampling cadence.

use easymanage_protocol::{PlatformProcessMetric, ServiceResourceMetric};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const MAX_PROC_FILE_BYTES: u64 = 16 * 1024;

#[derive(Default)]
pub struct ProcessSampler {
    previous: Option<Reading>,
}

struct Reading {
    sampled_at: Instant,
    process_ticks: Option<u64>,
    children_ticks: Option<u64>,
    host_ticks: Option<u64>,
    io_bytes: Option<(u64, u64)>,
    logical_bytes: Option<(u64, u64)>,
    service: Option<ServiceReading>,
}

struct ServiceReading {
    unit: String,
    usage_usec: Option<u64>,
    memory: Option<u64>,
    io: Option<(u64, u64)>,
}

impl ProcessSampler {
    pub fn sample(&mut self) -> PlatformProcessMetric {
        let now = Instant::now();
        let stat = read_bounded("/proc/self/stat");
        let process_ticks = stat.as_deref().and_then(process_ticks);
        let children_ticks = stat.as_deref().and_then(|stat| {
            let fields: Vec<_> = stat.rsplit_once(") ")?.1.split_whitespace().collect();
            fields
                .get(13)?
                .parse::<u64>()
                .ok()?
                .checked_add(fields.get(14)?.parse().ok()?)
        });
        let host_ticks = read_host_ticks();
        let io = read_bounded("/proc/self/io");
        let io_bytes = io.as_deref().and_then(io_bytes);
        let logical_bytes = io
            .as_deref()
            .and_then(|data| Some((counter(data, "rchar:")?, counter(data, "wchar:")?)));
        let service = service_reading();
        let resident_memory_bytes = read_bounded("/proc/self/status")
            .as_deref()
            .and_then(resident_bytes);
        let cpu_percent_of_host = self.previous.as_ref().and_then(|previous| {
            let elapsed = now.duration_since(previous.sampled_at);
            if elapsed < Duration::from_secs(1) {
                return None;
            }
            let used = process_ticks?.checked_sub(previous.process_ticks?)?;
            let total = host_ticks?.checked_sub(previous.host_ticks?)?;
            (total > 0).then_some((used as f64 / total as f64 * 100.0).min(100.0))
        });
        let io_rates = self.previous.as_ref().and_then(|previous| {
            let elapsed = now.duration_since(previous.sampled_at).as_secs_f64();
            if elapsed < 1.0 {
                return None;
            }
            let (read, written) = io_bytes?;
            let (previous_read, previous_written) = previous.io_bytes?;
            Some((
                read.checked_sub(previous_read)? as f64 / elapsed,
                written.checked_sub(previous_written)? as f64 / elapsed,
            ))
        });
        let elapsed = self
            .previous
            .as_ref()
            .map(|old| now.duration_since(old.sampled_at).as_secs_f64())
            .filter(|seconds| *seconds >= 1.0);
        let hz = tick_hz().map(|hz| hz as f64);
        let cpu_one_core = delta(
            process_ticks,
            self.previous.as_ref().and_then(|old| old.process_ticks),
            elapsed,
        )
        .zip(hz)
        .map(|(ticks, hz)| ticks / hz * 100.0);
        let children_cpu = delta(
            children_ticks,
            self.previous.as_ref().and_then(|old| old.children_ticks),
            elapsed,
        )
        .zip(hz)
        .map(|(ticks, hz)| ticks / hz * 100.0);
        let logical_rates = pair_rates(
            logical_bytes,
            self.previous.as_ref().and_then(|old| old.logical_bytes),
            elapsed,
        );
        let service_metric = service.as_ref().map(|sample| {
            let previous = self
                .previous
                .as_ref()
                .and_then(|old| old.service.as_ref())
                .filter(|old| old.unit == sample.unit);
            let rates = pair_rates(sample.io, previous.and_then(|old| old.io), elapsed);
            ServiceResourceMetric {
                unit: sample.unit.clone(),
                cpu_percent_of_one_core: delta(
                    sample.usage_usec,
                    previous.and_then(|old| old.usage_usec),
                    elapsed,
                )
                .map(|value| value / 10_000.0),
                memory_current_bytes: sample.memory,
                disk_read_bytes_per_second: rates.map(|pair| pair.0),
                disk_written_bytes_per_second: rates.map(|pair| pair.1),
            }
        });
        self.previous = Some(Reading {
            sampled_at: now,
            process_ticks,
            children_ticks,
            host_ticks,
            io_bytes,
            logical_bytes,
            service,
        });
        PlatformProcessMetric {
            sample_interval_seconds: elapsed,
            cpu_percent_of_one_core: cpu_one_core,
            children_cpu_percent_of_one_core: children_cpu,
            logical_read_bytes_per_second: logical_rates.map(|pair| pair.0),
            logical_written_bytes_per_second: logical_rates.map(|pair| pair.1),
            service: service_metric,
            pid: std::process::id(),
            cpu_percent_of_host,
            resident_memory_bytes,
            disk_read_bytes_per_second: io_rates.map(|rates| rates.0),
            disk_written_bytes_per_second: io_rates.map(|rates| rates.1),
        }
    }
}

fn delta(value: Option<u64>, old: Option<u64>, elapsed: Option<f64>) -> Option<f64> {
    Some(value?.checked_sub(old?)? as f64 / elapsed?)
}
fn pair_rates(
    value: Option<(u64, u64)>,
    old: Option<(u64, u64)>,
    elapsed: Option<f64>,
) -> Option<(f64, f64)> {
    let (read, written) = value?;
    let (old_read, old_written) = old?;
    Some((
        delta(Some(read), Some(old_read), elapsed)?,
        delta(Some(written), Some(old_written), elapsed)?,
    ))
}
fn counter(data: &str, key: &str) -> Option<u64> {
    data.lines()
        .find_map(|line| line.strip_prefix(key)?.trim().parse().ok())
}
fn tick_hz() -> Option<u64> {
    static HZ: OnceLock<Option<u64>> = OnceLock::new();
    *HZ.get_or_init(|| {
        // Linux AT_CLKTCK (17), read once without a getconf subprocess or unsafe sysconf.
        let bytes = std::fs::read("/proc/self/auxv").ok()?;
        if bytes.len() > MAX_PROC_FILE_BYTES as usize {
            return None;
        }
        let size = std::mem::size_of::<usize>();
        bytes.chunks_exact(2 * size).find_map(|entry| {
            let key = usize::from_ne_bytes(entry[..size].try_into().ok()?);
            let value = usize::from_ne_bytes(entry[size..].try_into().ok()?);
            (key == 17 && value > 0).then_some(value as u64)
        })
    })
}
fn service_path(data: &str) -> Option<(PathBuf, String)> {
    let path = data.lines().find_map(|line| line.strip_prefix("0::"))?;
    let mut result = PathBuf::from("/sys/fs/cgroup");
    for component in Path::new(path).components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                result.push(name);
                let name = name.to_str()?;
                if name.starts_with("easymanage-") && name.ends_with(".service") {
                    return Some((result, name.to_owned()));
                }
            }
            _ => return None,
        }
    }
    None
}
fn service_reading() -> Option<ServiceReading> {
    let (path, unit) = service_path(&read_bounded("/proc/self/cgroup")?)?;
    let read = |name: &str| read_bounded(path.join(name).to_str()?);
    let usage_usec = read("cpu.stat")
        .as_deref()
        .and_then(|data| counter(data, "usage_usec "));
    let memory = read("memory.current").and_then(|value| value.trim().parse().ok());
    let io = read("io.stat").as_deref().and_then(service_io);
    Some(ServiceReading {
        unit,
        usage_usec,
        memory,
        io,
    })
}
fn service_io(data: &str) -> Option<(u64, u64)> {
    data.lines()
        .try_fold((0_u64, 0_u64), |(read, written), line| {
            let get = |key: &str| {
                line.split_whitespace()
                    .find_map(|part| part.strip_prefix(key)?.parse::<u64>().ok())
            };
            Some((
                read.checked_add(get("rbytes=")?)?,
                written.checked_add(get("wbytes=")?)?,
            ))
        })
}

fn read_bounded(path: &str) -> Option<String> {
    let mut contents = String::new();
    File::open(path)
        .ok()?
        .take(MAX_PROC_FILE_BYTES + 1)
        .read_to_string(&mut contents)
        .ok()?;
    (contents.len() as u64 <= MAX_PROC_FILE_BYTES).then_some(contents)
}

fn read_host_ticks() -> Option<u64> {
    let file = File::open("/proc/stat").ok()?;
    let mut first_line = String::new();
    BufReader::new(file.take(MAX_PROC_FILE_BYTES + 1))
        .read_line(&mut first_line)
        .ok()?;
    (first_line.len() as u64 <= MAX_PROC_FILE_BYTES)
        .then(|| host_ticks(&first_line))
        .flatten()
}

fn process_ticks(stat: &str) -> Option<u64> {
    // The command name in parentheses may itself contain spaces or ')'.
    let fields: Vec<&str> = stat.rsplit_once(") ")?.1.split_whitespace().collect();
    fields
        .get(11)?
        .parse::<u64>()
        .ok()?
        .checked_add(fields.get(12)?.parse::<u64>().ok()?)
}

fn host_ticks(stat: &str) -> Option<u64> {
    let mut fields = stat.lines().next()?.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    fields.take(8).try_fold(0_u64, |total, value| {
        total.checked_add(value.parse::<u64>().ok()?)
    })
}

fn resident_bytes(status: &str) -> Option<u64> {
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let mut fields = line.split_whitespace();
    if fields.next()? != "VmRSS:" {
        return None;
    }
    let kilobytes = fields.next()?.parse::<u64>().ok()?;
    if fields.next()? != "kB" {
        return None;
    }
    kilobytes.checked_mul(1024)
}

fn io_bytes(io: &str) -> Option<(u64, u64)> {
    let parse = |name: &str| {
        io.lines()
            .find_map(|line| line.strip_prefix(name)?.trim().parse::<u64>().ok())
    };
    Some((parse("read_bytes:")?, parse("write_bytes:")?))
}

#[cfg(test)]
mod tests {
    use super::{host_ticks, io_bytes, process_ticks, resident_bytes};

    #[test]
    fn service_accounting_rejects_shared_sessions_and_counter_resets() {
        use super::*;
        assert!(service_path("0::/user.slice/user-1000.slice/session-1.scope").is_none());
        assert!(service_path("0::/system.slice/postgresql.service").is_none());
        assert!(service_path("0::/../easymanage-server.service").is_none());
        assert_eq!(
            service_path("0::/user.slice/app.slice/easymanage-server.service")
                .unwrap()
                .1,
            "easymanage-server.service"
        );
        assert_eq!(
            service_io("8:0 rbytes=12 wbytes=40 rios=1\n8:1 rbytes=8 wbytes=2"),
            Some((20, 42))
        );
        assert_eq!(service_io(""), Some((0, 0)));
        assert_eq!(service_io("8:0 rbytes=no"), None);
        assert_eq!(delta(Some(500), Some(100), Some(2.0)), Some(200.0));
        assert_eq!(delta(Some(1), Some(100), Some(2.0)), None);
        assert_eq!(delta(Some(1), Some(0), None), None);
    }

    #[test]
    fn bounded_proc_parsers_handle_process_names_and_counters() {
        let stat = format!(
            "321 (an odd) name) S {}",
            (1..=30)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        );
        assert_eq!(process_ticks(&stat), Some(23));
        assert_eq!(host_ticks("cpu  1 2 3 4 5 6 7 8 100 200\n"), Some(36));
        assert_eq!(
            resident_bytes("Name:\teasymanage\nVmRSS:\t1234 kB\n"),
            Some(1_263_616)
        );
        assert_eq!(
            io_bytes("read_bytes: 42\nwrite_bytes: 99\n"),
            Some((42, 99))
        );
        assert_eq!(resident_bytes("VmRSS: nope kB"), None);
        assert_eq!(io_bytes("read_bytes: 42\n"), None);
    }
}
