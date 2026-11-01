use easymanage_protocol::GpuMetric;
use std::io;
use tracing::debug;

const MEBIBYTE: u64 = 1024 * 1024;

#[derive(Default)]
pub struct Sampler {
    legacy_only: bool,
}

impl Sampler {
    pub async fn collect(&mut self) -> (Vec<GpuMetric>, Option<String>) {
        let mut result = query_nvidia(!self.legacy_only).await;
        // An older CLI may reject the additional fields. Fall back once per
        // process; timeouts/not-found never trigger an extra subprocess.
        if !self.legacy_only
            && result
                .as_ref()
                .is_err_and(|error| error.kind() == io::ErrorKind::Other)
        {
            self.legacy_only = true;
            result = query_nvidia(false).await;
        }
        match result {
            Ok(metrics) => metrics,
            Err(error) if error.kind() == io::ErrorKind::NotFound => (
                Vec::new(),
                Some("未安装 nvidia-smi，GPU 指标不可用".to_owned()),
            ),
            Err(error) => {
                debug!(%error, "GPU metrics are unavailable");
                (Vec::new(), Some(format!("GPU 指标不可用：{error}")))
            }
        }
    }
}

async fn query_nvidia(with_throttling: bool) -> io::Result<(Vec<GpuMetric>, Option<String>)> {
    let query = if with_throttling {
        "--query-gpu=index,name,utilization.gpu,memory.total,memory.used,temperature.gpu,power.draw,clocks_throttle_reasons.supported,clocks_throttle_reasons.active"
    } else {
        "--query-gpu=index,name,utilization.gpu,memory.total,memory.used,temperature.gpu,power.draw"
    };
    let stdout = crate::platform::common::command::read_command(
        "nvidia-smi",
        &[query, "--format=csv,noheader,nounits"],
        256 * 1024,
    )
    .await?;
    let mut metrics = Vec::new();
    let mut unavailable = Vec::new();
    for (index, line) in stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
    {
        if let Some(metric) = parse_line(line) {
            metrics.push(metric);
        } else {
            unavailable.push((index + 1).to_string());
        }
    }
    let issue = if unavailable.is_empty() {
        None
    } else {
        Some(format!(
            "GPU 查询第 {} 行缺少有效利用率/显存读数（例如 N/A），已跳过，未按 0 上报",
            unavailable.join("、")
        ))
    };
    Ok((metrics, issue))
}

fn parse_line(line: &str) -> Option<GpuMetric> {
    let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
    if !matches!(fields.len(), 7 | 9) {
        return None;
    }

    Some(GpuMetric {
        index: parse_number(fields[0])?,
        name: fields[1].to_owned(),
        utilization_percent: parse_number::<f32>(fields[2])
            .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))?,
        memory_total_bytes: mib_to_bytes(fields[3]).filter(|bytes| *bytes > 0)?,
        memory_used_bytes: mib_to_bytes(fields[4])?,
        temperature_celsius: parse_number::<f32>(fields[5]).filter(|value| value.is_finite()),
        power_watts: parse_number::<f32>(fields[6]).filter(|value| value.is_finite()),
        thermal_throttling: if fields.len() == 9 {
            thermal_throttling(fields[7], fields[8])
        } else {
            None
        },
    })
}

fn thermal_throttling(supported: &str, active: &str) -> Option<bool> {
    // NVML SW thermal slowdown (0x20) and HW thermal slowdown (0x40).
    // Idle, power caps, power brakes and generic HW slowdown do not qualify.
    const THERMAL: u64 = 0x20 | 0x40;
    let mask = |value: &str| {
        u64::from_str_radix(
            value
                .trim()
                .strip_prefix("0x")
                .or_else(|| value.trim().strip_prefix("0X"))?,
            16,
        )
        .ok()
    };
    let active = mask(active)?;
    if active & THERMAL != 0 {
        return Some(true);
    }
    (mask(supported)? & THERMAL == THERMAL).then_some(false)
}

fn mib_to_bytes(value: &str) -> Option<u64> {
    parse_number::<u64>(value).and_then(|mib| mib.checked_mul(MEBIBYTE))
}

fn parse_number<T: std::str::FromStr>(value: &str) -> Option<T> {
    if value.eq_ignore_ascii_case("n/a") || value.is_empty() {
        None
    } else {
        value.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::parse_line;

    #[test]
    fn unavailable_required_gpu_metrics_are_not_reported_as_idle() {
        assert!(parse_line("0, NVIDIA GPU, N/A, 32768, 0, 35, 20").is_none());
        assert!(parse_line("0, NVIDIA GPU, 0, N/A, 0, 35, 20").is_none());
        assert!(parse_line("0, NVIDIA GPU, 0, 32768, N/A, 35, 20").is_none());
        assert!(parse_line("0, NVIDIA GPU, NaN, 32768, 0, 35, 20").is_none());
        let idle = parse_line("0, NVIDIA GPU, 0, 32768, 0, N/A, N/A")
            .expect("known idle values are valid");
        assert!(idle.utilization_percent.abs() < f32::EPSILON);
        assert_eq!(idle.memory_used_bytes, 0);
        assert_eq!(idle.memory_total_bytes, 32768 * 1024 * 1024);
        assert_eq!(idle.temperature_celsius, None);
        assert_eq!(idle.power_watts, None);
        assert_eq!(idle.thermal_throttling, None);
    }

    #[test]
    fn only_specific_thermal_clock_reasons_indicate_overheating() {
        for active in ["0x20", "0x40", "0x60", "0x0000000000000024"] {
            assert_eq!(super::thermal_throttling("0x1ff", active), Some(true));
        }
        for active in ["0x0", "0x1", "0x4", "0x8", "0x80", "0x9c"] {
            assert_eq!(super::thermal_throttling("0x1ff", active), Some(false));
        }
        assert_eq!(super::thermal_throttling("N/A", "N/A"), None);
        assert_eq!(super::thermal_throttling("0x4", "0x4"), None);
        assert_eq!(
            parse_line("0, GPU, 90, 24564, 20000, 95, 300, 0x1ff, 0x4")
                .unwrap()
                .thermal_throttling,
            Some(false)
        );
        assert_eq!(
            parse_line("0, GPU, 90, 24564, 20000, 65, 300, 0x1ff, 0x20")
                .unwrap()
                .thermal_throttling,
            Some(true)
        );
    }
}
