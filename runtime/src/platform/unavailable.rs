//! No Linux /proc fallback on another OS. Native accounting remains to be added.
use easymanage_protocol::PlatformProcessMetric;

#[derive(Default)]
pub struct ProcessSampler;

impl ProcessSampler {
    pub fn sample(&mut self) -> PlatformProcessMetric {
        PlatformProcessMetric {
            pid: std::process::id(),
            cpu_percent_of_host: None,
            resident_memory_bytes: None,
            disk_read_bytes_per_second: None,
            disk_written_bytes_per_second: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProcessSampler;

    #[test]
    fn missing_native_sampler_returns_unknown_not_zero() {
        let metric = ProcessSampler.sample();
        assert_eq!(metric.pid, std::process::id());
        assert_eq!(metric.cpu_percent_of_host, None);
        assert_eq!(metric.resident_memory_bytes, None);
        assert_eq!(metric.disk_read_bytes_per_second, None);
        assert_eq!(metric.disk_written_bytes_per_second, None);
    }
}
