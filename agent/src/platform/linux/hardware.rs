//! Daily, bounded reads of kernel-exposed inventory; no probing utilities or /dev/mem.
use chrono::{DateTime, Utc};
use easymanage_protocol::{BlockDevice, GpuMetric, HardwareDetails, NodeInventory};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::Duration,
};

pub const INTERVAL: Duration = Duration::from_secs(86_400);

pub fn cache_age(at: DateTime<Utc>, now: DateTime<Utc>) -> Option<Duration> {
    (now - at).to_std().ok().filter(|age| *age < INTERVAL)
}

fn bytes(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let mut result = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut result)?;
    if result.len() as u64 > limit {
        return Err(io::Error::other("hardware file exceeds limit"));
    }
    Ok(result)
}

fn clean(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(240)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn text(path: &Path) -> Option<String> {
    let value = clean(&String::from_utf8_lossy(&bytes(path, 4096).ok()?));
    if value.is_empty()
        || matches!(
            value.to_ascii_lowercase().as_str(),
            "unknown" | "not specified" | "default string" | "to be filled by o.e.m."
        )
    {
        None
    } else {
        Some(value)
    }
}

fn entries(path: &str, limit: usize) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(path)
        .into_iter()
        .flatten()
        .take(limit)
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn capacity(size: u64) -> String {
    format!("{:.1} GiB", size as f64 / 1_073_741_824.0)
}
fn word(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(offset..offset + 2)?.try_into().ok()?,
    ))
}
fn dword(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

// SMBIOS type 17 offsets: size 0x0c, locator 0x10, type 0x12, speed 0x15,
// manufacturer 0x17, part 0x1a, extended size 0x1c, configured speed 0x20.
// Serial numbers and asset tags are deliberately never exported.
fn memory_modules(table: &[u8]) -> Result<Vec<String>, &'static str> {
    let mut offset = 0;
    let mut modules = Vec::new();
    for _ in 0..4096 {
        if offset == table.len() {
            return Ok(modules);
        }
        let header = table.get(offset..offset + 4).ok_or("DMI 表不完整")?;
        let length = usize::from(header[1]);
        if length < 4 {
            return Err("DMI 记录长度无效");
        }
        let data = table.get(offset..offset + length).ok_or("DMI 记录不完整")?;
        let strings = table.get(offset + length..).ok_or("DMI 字符串不完整")?;
        let end = strings
            .windows(2)
            .position(|pair| pair == [0, 0])
            .ok_or("DMI 字符串未终止")?;
        if header[0] == 127 {
            return Ok(modules);
        }
        if header[0] == 17 {
            if data.len() < 0x15 {
                return Err("内存设备记录不完整");
            }
            let size = word(data, 0x0c).unwrap_or(0xffff);
            if size != 0 {
                if modules.len() >= 128 {
                    return Err("内存设备数超过采集上限");
                }
                let get_string = |position: usize| -> Option<String> {
                    let index = usize::from(*data.get(position)?);
                    if index == 0 {
                        return None;
                    }
                    let value = clean(&String::from_utf8_lossy(
                        strings[..end].split(|byte| *byte == 0).nth(index - 1)?,
                    ));
                    (!value.is_empty() && value != "Unknown").then_some(value)
                };
                let size_bytes = match size {
                    0xffff => None,
                    0x7fff => dword(data, 0x1c)
                        .map(|value| u64::from(value & 0x7fff_ffff) * 1_048_576)
                        .filter(|size| *size > 0),
                    size if size & 0x8000 != 0 => Some(u64::from(size & 0x7fff) * 1024),
                    size => Some(u64::from(size) * 1_048_576),
                };
                let kind = match data[0x12] {
                    0x12 => "DDR",
                    0x13 => "DDR2",
                    0x14 => "DDR2 FB-DIMM",
                    0x18 => "DDR3",
                    0x1a => "DDR4",
                    0x1b => "LPDDR",
                    0x1c => "LPDDR2",
                    0x1d => "LPDDR3",
                    0x1e => "LPDDR4",
                    0x22 => "DDR5",
                    0x23 => "LPDDR5",
                    _ => "内存类型未知",
                };
                let speed = |offset, extended| match word(data, offset) {
                    Some(0) | None => None,
                    Some(0xffff) => dword(data, extended).filter(|value| *value > 0),
                    Some(value) => Some(u32::from(value)),
                };
                let configured = speed(0x20, 0x58);
                let speed = configured.or_else(|| speed(0x15, 0x54));
                let mut parts = Vec::new();
                if let Some(value) = get_string(0x17) {
                    parts.push(value);
                }
                if let Some(value) = get_string(0x1a) {
                    parts.push(value);
                }
                parts.push(
                    size_bytes
                        .map(capacity)
                        .unwrap_or_else(|| "容量未知".into()),
                );
                parts.push(kind.into());
                if let Some(value) = speed {
                    parts.push(format!(
                        "{value} MT/s（{}）",
                        if configured.is_some() {
                            "配置速率"
                        } else {
                            "额定速率"
                        }
                    ));
                }
                if let Some(value) = get_string(0x10) {
                    parts.push(value);
                }
                modules.push(clean(&parts.join(" · ")));
            }
        }
        offset += length + end + 2;
    }
    Err("DMI 记录数超过采集上限")
}

// Decode only the validated base EDID block exposed by DRM (no DDC/I2C requests).
// Layout: https://github.com/torvalds/linux/blob/master/include/drm/drm_edid.h
fn display_edid(data: &[u8]) -> Option<String> {
    let data = data.get(..128)?;
    if data[..8] != [0, 255, 255, 255, 255, 255, 255, 0]
        || data.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) != 0
    {
        return None;
    }
    let vendor = u16::from_be_bytes([data[8], data[9]]);
    let letters = [
        ((vendor >> 10) & 31) as u8,
        ((vendor >> 5) & 31) as u8,
        (vendor & 31) as u8,
    ];
    if letters.iter().any(|letter| !(1..=26).contains(letter)) {
        return None;
    }
    let vendor: String = letters
        .into_iter()
        .map(|letter| char::from(b'A' + letter - 1))
        .collect();
    let mut model = None;
    for descriptor in data[54..126].chunks_exact(18) {
        if descriptor[..4] == [0, 0, 0, 0xfc] {
            model = Some(clean(&String::from_utf8_lossy(&descriptor[5..18])));
        }
    }
    let mut label = format!(
        "{} [{vendor}-{:04X}]",
        model
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "显示器".into()),
        word(data, 10)?
    );
    if data[21] > 0 && data[22] > 0 {
        let inches = f64::from(data[21]).hypot(f64::from(data[22])) / 2.54;
        label.push_str(&format!(" · {inches:.1} 英寸"));
    }
    Some(label)
}

struct PciDevice {
    address: String,
    vendor: String,
    class: u32,
    name: String,
}

fn pci_names(source: &str) -> BTreeMap<(String, String), String> {
    let mut names = BTreeMap::new();
    let mut vendor = None::<(String, String)>;
    for line in source.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            vendor = line
                .split_once("  ")
                .filter(|(id, _)| id.len() == 4 && id.bytes().all(|b| b.is_ascii_hexdigit()))
                .map(|(id, name)| (id.to_ascii_lowercase(), clean(name)));
        } else if !line.starts_with("\t\t")
            && let Some((id, name)) = line[1..].split_once("  ")
            && id.len() == 4
            && id.bytes().all(|b| b.is_ascii_hexdigit())
            && let Some((vendor_id, vendor_name)) = &vendor
        {
            names.insert(
                (vendor_id.clone(), id.to_ascii_lowercase()),
                clean(&format!("{vendor_name} {name}")),
            );
        }
    }
    names
}

fn pci_devices() -> Vec<PciDevice> {
    let source = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"]
        .iter()
        .find_map(|path| bytes(Path::new(path), 4 * 1024 * 1024).ok())
        .unwrap_or_default();
    let names = pci_names(&String::from_utf8_lossy(&source));
    entries("/sys/bus/pci/devices", 4096)
        .into_iter()
        .filter_map(|path| {
            let class = text(&path.join("class"))
                .and_then(|value| u32::from_str_radix(value.trim_start_matches("0x"), 16).ok())?;
            // Display, network and audio only; no configuration-space reads.
            if !matches!(class >> 16, 2 | 3) && !matches!(class >> 8, 0x0401 | 0x0403) {
                return None;
            }
            let vendor = text(&path.join("vendor"))?
                .trim_start_matches("0x")
                .to_ascii_lowercase();
            let device = text(&path.join("device"))?
                .trim_start_matches("0x")
                .to_ascii_lowercase();
            let name = names
                .get(&(vendor.clone(), device.clone()))
                .cloned()
                .unwrap_or_else(|| format!("PCI {vendor}:{device}（型号库未识别）"));
            Some(PciDevice {
                address: path.file_name()?.to_string_lossy().into_owned(),
                vendor,
                class,
                name,
            })
        })
        .collect()
}

pub fn collect(
    inventory: Option<NodeInventory>,
    memory_total: u64,
    gpus: &[GpuMetric],
    disks: &[BlockDevice],
) -> HardwareDetails {
    let mut result = HardwareDetails {
        collected_at: Utc::now(),
        processors: vec![],
        motherboard: vec![],
        memory: vec![],
        graphics: vec![],
        displays: vec![],
        disks: vec![],
        audio: vec![],
        network: vec![],
        notes: vec![],
    };
    if let Some(cpu) = inventory {
        result.processors.push(format!(
            "{} · {} · {} 线程{}",
            cpu.cpu_model,
            cpu.architecture,
            cpu.cpu_logical_cores,
            cpu.cpu_physical_cores
                .map(|cores| format!(" / {cores} 物理核心"))
                .unwrap_or_default()
        ));
    }
    let board = ["board_vendor", "board_name", "board_version"]
        .into_iter()
        .filter_map(|name| text(&Path::new("/sys/class/dmi/id").join(name)))
        .collect::<Vec<_>>();
    if !board.is_empty() {
        result.motherboard.push(board.join(" · "));
    } else {
        result
            .notes
            .push("主板：系统未提供可读取的 DMI 型号。".into());
    }
    result
        .memory
        .push(format!("系统可用物理总量 {}", capacity(memory_total)));
    match bytes(Path::new("/sys/firmware/dmi/tables/DMI"), 1024 * 1024) {
        Ok(table) => match memory_modules(&table) {
            Ok(modules) if !modules.is_empty() => result.memory.extend(modules),
            Ok(_) => result
                .notes
                .push("内存：固件没有提供已安装内存条记录。".into()),
            Err(reason) => result.notes.push(format!("内存条详情不可用：{reason}。")),
        },
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => result
            .notes
            .push("内存条品牌、规格、插槽：读取 DMI 需要 Full 权限。".into()),
        Err(_) => result
            .notes
            .push("内存条品牌、规格、插槽：系统未提供可读取的 DMI 表。".into()),
    }
    let pci = pci_devices();
    for gpu in gpus.iter().take(128) {
        result.graphics.push(format!(
            "GPU {} · {} · 显存 {}",
            gpu.index,
            gpu.name,
            capacity(gpu.memory_total_bytes)
        ));
    }
    for device in pci.iter().filter(|device| device.class >> 16 == 3) {
        if device.vendor != "10de" || gpus.is_empty() {
            result
                .graphics
                .push(format!("{} · {}", device.name, device.address));
        }
    }
    for connector in entries("/sys/class/drm", 128) {
        if text(&connector.join("status")).as_deref() != Some("connected") {
            continue;
        }
        let name = connector.file_name().unwrap_or_default().to_string_lossy();
        let edid = bytes(&connector.join("edid"), 32 * 1024)
            .ok()
            .and_then(|data| display_edid(&data));
        result.displays.push(format!(
            "{name} · {}",
            edid.unwrap_or_else(|| "已连接，EDID 型号不可读取".into())
        ));
    }
    if result.displays.is_empty() {
        result.notes.push(
            "显示器：未发现内核上报的已连接显示器，可能为无屏服务器或驱动未提供 EDID。".into(),
        );
    }
    for disk in disks.iter().filter(|disk| disk.kind == "disk").take(128) {
        result.disks.push(format!(
            "{} · {} · {}{}",
            disk.model.as_deref().unwrap_or("型号未知"),
            capacity(disk.size_bytes),
            disk.path.as_deref().unwrap_or(&disk.name),
            disk.transport
                .as_ref()
                .map(|transport| format!(" · {transport}"))
                .unwrap_or_default()
        ));
    }
    if let Ok(cards) = bytes(Path::new("/proc/asound/cards"), 64 * 1024) {
        result.audio = String::from_utf8_lossy(&cards)
            .lines()
            .filter_map(|line| {
                let (id, rest) = line.trim().split_once(' ')?;
                id.parse::<u32>().ok()?;
                let (_, name) = rest.split_once("]:")?;
                Some(format!("声卡 {id} · {}", clean(name)))
            })
            .take(128)
            .collect();
    }
    if result.audio.is_empty() {
        result.audio = pci
            .iter()
            .filter(|device| matches!(device.class >> 8, 0x0401 | 0x0403))
            .map(|device| format!("{} · {}", device.name, device.address))
            .collect();
    }
    let mut linked = HashSet::new();
    for interface in entries("/sys/class/net", 256) {
        let Ok(path) = fs::canonicalize(interface.join("device")) else {
            continue;
        };
        let address = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let model = pci
            .iter()
            .find(|device| device.address == address)
            .map(|device| {
                linked.insert(address.clone());
                device.name.clone()
            })
            .or_else(|| {
                path.ancestors()
                    .take(3)
                    .find_map(|parent| text(&parent.join("product")))
            })
            .unwrap_or_else(|| "型号未知".into());
        let driver = fs::read_link(interface.join("device/driver"))
            .ok()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            });
        let name = interface.file_name().unwrap_or_default().to_string_lossy();
        result.network.push(format!(
            "{name} · {model} · {}{}",
            if interface.join("wireless").exists() {
                "Wi-Fi"
            } else {
                "物理网卡"
            },
            driver
                .map(|driver| format!(" · {driver}"))
                .unwrap_or_default()
        ));
    }
    for device in pci
        .iter()
        .filter(|device| device.class >> 16 == 2 && !linked.contains(&device.address))
    {
        result.network.push(format!(
            "{} · {}（未发现关联网络接口）",
            device.name, device.address
        ));
    }
    for items in [
        &mut result.processors,
        &mut result.motherboard,
        &mut result.memory,
        &mut result.graphics,
        &mut result.displays,
        &mut result.disks,
        &mut result.audio,
        &mut result.network,
        &mut result.notes,
    ] {
        if items.len() > 128 {
            items.truncate(128);
        }
        for item in items {
            *item = clean(item);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dmi_memory_decodes_size_speed_and_omits_serials() {
        let mut table = vec![0u8; 0x5c];
        table[0] = 17;
        table[1] = 0x5c;
        table[0x0c..0x0e].copy_from_slice(&0x7fff_u16.to_le_bytes());
        table[0x1c..0x20].copy_from_slice(&32768_u32.to_le_bytes());
        table[0x10] = 1;
        table[0x17] = 2;
        table[0x18] = 3;
        table[0x1a] = 4;
        table[0x12] = 0x22;
        table[0x20..0x22].copy_from_slice(&4800_u16.to_le_bytes());
        table.extend_from_slice(b"DIMM_A1\0Kingston\0SECRET_SERIAL\0MODEL\0\0");
        let modules = memory_modules(&table).unwrap();
        assert_eq!(modules.len(), 1);
        assert!(modules[0].contains("32.0 GiB · DDR5 · 4800 MT/s"));
        assert!(!modules[0].contains("SECRET_SERIAL"));
        table[0x0c..0x0e].copy_from_slice(&0u16.to_le_bytes());
        assert!(memory_modules(&table).unwrap().is_empty());
        assert!(memory_modules(&[17, 2, 0, 0]).is_err());
        assert!(memory_modules(&table[..table.len() - 1]).is_err());
    }

    #[test]
    fn edid_requires_valid_checksum_and_retains_model_and_size_only() {
        let mut data = [0u8; 128];
        data[..8].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 0]);
        data[8..10].copy_from_slice(&0x410cu16.to_be_bytes()); // PHL
        data[10..12].copy_from_slice(&0x0940u16.to_le_bytes());
        data[21] = 60;
        data[22] = 34;
        data[57] = 0xfc;
        data[59..72].copy_from_slice(b"PHL 272S9\n   ");
        data[127] = 0u8.wrapping_sub(data.iter().fold(0u8, |sum, b| sum.wrapping_add(*b)));
        let model = display_edid(&data).unwrap();
        assert!(model.contains("PHL 272S9 [PHL-0940]") && model.contains("27.2 英寸"));
        data[127] ^= 1;
        assert!(display_edid(&data).is_none());
        assert!(display_edid(&data[..64]).is_none());
    }

    #[test]
    fn daily_cache_expires_and_rejects_future_timestamps() {
        let now = Utc::now();
        assert!(cache_age(now - chrono::Duration::hours(23), now).is_some());
        assert!(cache_age(now - chrono::Duration::hours(24), now).is_none());
        assert!(cache_age(now + chrono::Duration::seconds(1), now).is_none());
        let names = pci_names(
            "10de  NVIDIA\n\t1234  Fixture GPU\n\t\t0001 0002  Subsystem\nC 03  Display\n\t00  VGA\n",
        );
        assert_eq!(
            names.get(&("10de".into(), "1234".into())).unwrap(),
            "NVIDIA Fixture GPU"
        );
        assert_eq!(names.len(), 1);
    }
}
