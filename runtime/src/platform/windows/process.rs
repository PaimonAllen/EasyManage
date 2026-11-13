use easymanage_protocol::PlatformProcessMetric;
use std::time::Instant;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
#[derive(Default)]
pub struct ProcessSampler {
    system: System,
    last: Option<Instant>,
}
impl ProcessSampler {
    pub fn sample(&mut self) -> PlatformProcessMetric {
        let pid = Pid::from_u32(std::process::id());
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory()
                .with_disk_usage(),
        );
        let now = Instant::now();
        let elapsed = self
            .last
            .replace(now)
            .map(|last| now.duration_since(last).as_secs_f64())
            .filter(|s| *s > 0.0);
        let process = self.system.process(pid);
        let cores = std::thread::available_parallelism().map_or(1, usize::from) as f64;
        PlatformProcessMetric {
            pid: pid.as_u32(),
            sample_interval_seconds: elapsed,
            cpu_percent_of_one_core: elapsed
                .and_then(|_| process.map(|p| f64::from(p.cpu_usage()))),
            cpu_percent_of_host: elapsed
                .and_then(|_| process.map(|p| f64::from(p.cpu_usage()) / cores)),
            resident_memory_bytes: process.map(|p| p.memory()),
            children_cpu_percent_of_one_core: None,
            logical_read_bytes_per_second: None,
            logical_written_bytes_per_second: None,
            service: None,
            disk_read_bytes_per_second: elapsed
                .and_then(|s| process.map(|p| p.disk_usage().read_bytes as f64 / s)),
            disk_written_bytes_per_second: elapsed
                .and_then(|s| process.map(|p| p.disk_usage().written_bytes as f64 / s)),
        }
    }
}
