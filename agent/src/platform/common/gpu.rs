use easymanage_protocol::GpuMetric;
use std::io;
use tracing::debug;

const MEBIBYTE: u64 = 1024 * 1024;

pub async fn collect() -> (Vec<GpuMetric>, Option<String>) {
    match query_nvidia().await {
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

async fn query_nvidia() -> io::Result<(Vec<GpuMetric>, Option<String>)> {
    let stdout = crate::platform::common::command::read_command("nvidia-smi", &[
            "--query-gpu=index,name,utilization.gpu,memory.total,memory.used,temperature.gpu,power.draw",
            "--format=csv,noheader,nounits",
        ], 256 * 1024).await?;
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
    if fields.len() != 7 {
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
    })
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
    }
}
