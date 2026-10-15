//! Bounded read-only hwmon collection. No SMART commands, probes, driver loading or writes.
use chrono::Utc;
use easymanage_protocol::{HostTemperatures, SsdTemperature};
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_DEVICES: usize = 128;
const MAX_CHANNELS: usize = 128;

fn read_small(path: &Path) -> Option<String> {
    let mut value = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(257)
        .read_to_string(&mut value)
        .ok()?;
    (value.len() <= 256).then(|| value.trim().to_owned())
}

fn reading(path: &Path) -> Option<f32> {
    let milli: i32 = read_small(path)?.parse().ok()?;
    // Reject sensor sentinels (including -273.15 C), not report them as idle.
    (0..=150_000)
        .contains(&milli)
        .then_some(milli as f32 / 1000.0)
}

fn maximum(values: impl Iterator<Item = f32>) -> Option<f32> {
    values.reduce(f32::max)
}

fn sensor_channels(path: &Path, driver: &str) -> Vec<(String, f32)> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    entries
        .take(MAX_CHANNELS * 8)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            let channel = name.strip_prefix("temp")?.strip_suffix("_input")?;
            let number = channel.parse::<usize>().ok()?;
            if number == 0 || number > MAX_CHANNELS {
                return None;
            }
            let stem = format!("temp{channel}");
            if read_small(&path.join(format!("{stem}_fault"))).is_some_and(|v| v != "0")
                || read_small(&path.join(format!("{stem}_enable"))).is_some_and(|v| v == "0")
            {
                return None;
            }
            let label = read_small(&path.join(format!("{stem}_label"))).unwrap_or_default();
            // AMD Tctl can include a non-physical control offset. Only accept Tdie/CCD.
            if matches!(driver, "k10temp" | "zenpower")
                && label != "Tdie"
                && !label.starts_with("Tccd")
            {
                return None;
            }
            Some((label, reading(&entry.path())?))
        })
        .collect()
}

fn ssd_device(sys: &Path, hwmon: &Path, driver: &str) -> Option<String> {
    let parent = std::fs::canonicalize(hwmon.join("device")).ok()?;
    if driver == "nvme" {
        let name = parent.file_name()?.to_str()?;
        let index = name.strip_prefix("nvme")?;
        if index.is_empty() || !index.bytes().all(|v| v.is_ascii_digit()) {
            return None;
        }
        // A controller, not a guessed PCI device or a partition.
        if std::fs::canonicalize(sys.join("class/nvme").join(name)).ok()? == parent {
            return Some(name.to_owned());
        }
        return None;
    }
    let blocks = std::fs::read_dir(sys.join("class/block")).ok()?;
    for entry in blocks.take(MAX_DEVICES).flatten() {
        let block = entry.path();
        if block.join("partition").exists()
            || read_small(&block.join("queue/rotational")).as_deref() != Some("0")
        {
            continue;
        }
        let device = std::fs::canonicalize(block.join("device")).ok();
        if device.as_ref() == Some(&parent) {
            return entry.file_name().to_str().map(str::to_owned);
        }
    }
    None
}

pub fn collect() -> HostTemperatures {
    collect_from(Path::new("/sys"))
}

fn collect_from(sys: &Path) -> HostTemperatures {
    let mut result = HostTemperatures {
        collected_at: Utc::now(),
        cpu_celsius: None,
        ssds: Vec::new(),
    };
    let Ok(entries) = std::fs::read_dir(sys.join("class/hwmon")) else {
        return result;
    };
    for entry in entries.take(MAX_DEVICES).flatten() {
        let path: PathBuf = entry.path();
        let Some(driver) = read_small(&path.join("name")) else {
            continue;
        };
        if matches!(driver.as_str(), "coretemp" | "k10temp" | "zenpower") {
            let channels = sensor_channels(&path, &driver);
            if let Some(value) = maximum(channels.into_iter().map(|(_, value)| value)) {
                result.cpu_celsius = Some(result.cpu_celsius.map_or(value, |old| old.max(value)));
            }
        } else if matches!(driver.as_str(), "nvme" | "drivetemp") {
            // Identify SSD BEFORE reading temperature. Never poll rotational disks.
            let Some(device) = ssd_device(sys, &path, &driver) else {
                continue;
            };
            if read_small(&path.join("temp1_fault")).is_some_and(|v| v != "0") {
                continue;
            }
            if read_small(&path.join("temp1_enable")).is_some_and(|v| v == "0") {
                continue;
            }
            if let Some(value) = reading(&path.join("temp1_input")) {
                result.ssds.push(SsdTemperature {
                    device,
                    temperature_celsius: value,
                });
            }
        }
    }
    result.ssds.sort_by(|a, b| a.device.cmp(&b.device));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn only_identified_cpu_and_ssd_sensors_are_read() {
        let root =
            std::env::temp_dir().join(format!("easymanage-thermal-{}", uuid::Uuid::new_v4()));
        let write = |relative: &str, value: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, value).unwrap();
        };
        write("class/hwmon/hwmon0/name", "coretemp");
        write("class/hwmon/hwmon0/temp1_input", "45000");
        write("class/hwmon/hwmon0/temp2_input", "51000");
        write("class/hwmon/hwmon0/temp3_input", "999999");
        write("class/hwmon/hwmon0/temp4_input", "95000");
        write("class/hwmon/hwmon0/temp4_fault", "1");
        write("class/hwmon/hwmon1/name", "nct6796");
        write("class/hwmon/hwmon1/temp1_input", "90000");
        write("class/hwmon/hwmon2/name", "nvme");
        write("class/hwmon/hwmon2/temp1_input", "38000");
        std::fs::create_dir_all(root.join("devices/nvme0")).unwrap();
        std::fs::create_dir_all(root.join("class/nvme")).unwrap();
        symlink(root.join("devices/nvme0"), root.join("class/nvme/nvme0")).unwrap();
        symlink(
            root.join("devices/nvme0"),
            root.join("class/hwmon/hwmon2/device"),
        )
        .unwrap();
        for (index, name, rotational) in [(3, "sda", "1"), (4, "sdb", "0")] {
            write(&format!("class/hwmon/hwmon{index}/name"), "drivetemp");
            write(&format!("class/hwmon/hwmon{index}/temp1_input"), "42000");
            write(&format!("class/block/{name}/queue/rotational"), rotational);
            std::fs::create_dir_all(root.join(format!("devices/{name}"))).unwrap();
            symlink(
                root.join(format!("devices/{name}")),
                root.join(format!("class/block/{name}/device")),
            )
            .unwrap();
            symlink(
                root.join(format!("devices/{name}")),
                root.join(format!("class/hwmon/hwmon{index}/device")),
            )
            .unwrap();
        }
        let result = collect_from(&root);
        assert_eq!(result.cpu_celsius, Some(51.0));
        assert_eq!(
            result
                .ssds
                .iter()
                .map(|s| (s.device.as_str(), s.temperature_celsius))
                .collect::<Vec<_>>(),
            vec![("nvme0", 38.0), ("sdb", 42.0)]
        );
        write("class/hwmon/hwmon0/name", "k10temp");
        write("class/hwmon/hwmon0/temp1_label", "Tctl");
        write("class/hwmon/hwmon0/temp2_label", "Tdie");
        assert_eq!(collect_from(&root).cpu_celsius, Some(51.0));
        write("class/hwmon/hwmon0/temp2_input", "-273150");
        assert_eq!(collect_from(&root).cpu_celsius, None);
        assert_eq!(collect_from(&root.join("absent")).cpu_celsius, None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
