//! Linux bounded, read-only accounting for the current EasyManage process.
//! No process enumeration or privileged tracing; callers choose the sampling cadence.

use easymanage_protocol::PlatformProcessMetric;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::time::{Duration, Instant};

const MAX_PROC_FILE_BYTES: u64 = 16 * 1024;

#[derive(Default)]
pub struct ProcessSampler {
    previous: Option<Reading>,
}

struct Reading {
    sampled_at: Instant,
    process_ticks: Option<u64>,
    host_ticks: Option<u64>,
    io_bytes: Option<(u64, u64)>,
}

impl ProcessSampler {
    pub fn sample(&mut self) -> PlatformProcessMetric {
        let now = Instant::now();
        let process_ticks = read_bounded("/proc/self/stat")
            .as_deref()
            .and_then(process_ticks);
        let host_ticks = read_host_ticks();
        let io_bytes = read_bounded("/proc/self/io").as_deref().and_then(io_bytes);
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
        self.previous = Some(Reading {
            sampled_at: now,
            process_ticks,
            host_ticks,
            io_bytes,
        });
        PlatformProcessMetric {
            pid: std::process::id(),
            cpu_percent_of_host,
            resident_memory_bytes,
            disk_read_bytes_per_second: io_rates.map(|rates| rates.0),
            disk_written_bytes_per_second: io_rates.map(|rates| rates.1),
        }
    }
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
