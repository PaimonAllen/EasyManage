use easymanage_protocol::BlockDevice;
use serde_json::Value;
use std::fs;
use std::io;
use std::path::Path;

const MAX_DEVICES: usize = 256;
const MAX_DEPTH: usize = 8;
const SECTOR_BYTES: u64 = 512;

pub fn parse(output: &str) -> io::Result<Vec<BlockDevice>> {
    let value: Value = serde_json::from_str(output).map_err(io::Error::other)?;
    let roots = value
        .get("blockdevices")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("lsblk returned no block device array"))?;
    let mut count = 0;
    let mut devices = Vec::new();
    for root in roots {
        // Snap/loop and optical media are not partitionable disks; keep the page focused.
        if matches!(
            root.get("type").and_then(Value::as_str),
            Some("loop" | "rom")
        ) {
            continue;
        }
        devices.push(parse_device(root, 0, &mut count)?);
    }
    Ok(devices)
}

fn parse_device(value: &Value, depth: usize, count: &mut usize) -> io::Result<BlockDevice> {
    if depth > MAX_DEPTH || *count >= MAX_DEVICES {
        return Err(io::Error::other("block device tree exceeds safety limit"));
    }
    *count += 1;
    let name = required_text(value, "name")?;
    let kind = required_text(value, "type")?;
    let size_bytes = value
        .get("size")
        .and_then(|field| field.as_u64().or_else(|| field.as_str()?.parse().ok()))
        .unwrap_or(0);
    let children = match value.get("children") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|child| parse_device(child, depth + 1, count))
            .collect::<io::Result<Vec<_>>>()?,
        _ => return Err(io::Error::other("invalid lsblk child list")),
    };
    let mount_points = value
        .get("mountpoints")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .filter(|point| !point.is_empty())
                .take(16)
                .map(|point| point.chars().take(256).collect())
                .collect()
        })
        .unwrap_or_default();
    let start_bytes = if kind == "part" {
        text(value, "kname").and_then(|kname| read_partition_start(&kname))
    } else {
        None
    };
    Ok(BlockDevice {
        name,
        path: text(value, "path"),
        kind,
        size_bytes,
        filesystem: text(value, "fstype"),
        label: text(value, "label"),
        mount_points,
        model: text(value, "model"),
        transport: text(value, "tran"),
        rotational: boolean(value, "rota"),
        removable: boolean(value, "rm"),
        read_only: boolean(value, "ro"),
        partition_table: text(value, "pttype"),
        partition_label: text(value, "partlabel"),
        start_bytes,
        children,
    })
}

fn required_text(value: &Value, key: &str) -> io::Result<String> {
    text(value, key).ok_or_else(|| io::Error::other(format!("lsblk {key} is missing")))
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .map(|field| field.chars().take(256).collect())
}

fn boolean(value: &Value, key: &str) -> Option<bool> {
    let field = value.get(key)?;
    field
        .as_bool()
        .or_else(|| field.as_u64().map(|number| number != 0))
}

fn read_partition_start(kname: &str) -> Option<u64> {
    if kname.is_empty()
        || matches!(kname, "." | "..")
        || !kname
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'!'))
    {
        return None;
    }
    let path = Path::new("/sys/class/block").join(kname).join("start");
    fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?
        .checked_mul(SECTOR_BYTES)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_unmounted_partitions_and_skips_snap_loops() {
        let output = r#"{"blockdevices":[
            {"name":"loop0","type":"loop","size":4096},
            {"name":"disk0","kname":"test-disk","path":"/dev/disk0","type":"disk","size":1000000,"ro":false,
             "children":[{"name":"disk0p1","kname":"missing-test-part","path":"/dev/disk0p1","type":"part","size":400000,"fstype":"ext4","mountpoints":["/data",null]},
                         {"name":"disk0p2","kname":"missing-test-part2","type":"part","size":200000,"fstype":null,"mountpoints":[null]}]}
        ]}"#;
        let roots = parse(output).expect("valid bounded lsblk tree");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].children.len(), 2);
        assert_eq!(roots[0].children[0].mount_points, ["/data"]);
        assert!(roots[0].children[1].mount_points.is_empty());
        assert_eq!(roots[0].children[1].size_bytes, 200000);
        assert_eq!(roots[0].children[1].start_bytes, None);
    }

    #[test]
    fn rejects_malformed_or_unbounded_trees() {
        assert!(parse("{}").is_err());
        assert!(
            parse(r#"{"blockdevices":[{"name":"disk0","type":"disk","children":{}}]}"#).is_err()
        );
        let roots = (0..257)
            .map(|index| format!(r#"{{"name":"disk{index}","type":"disk"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        assert!(parse(&format!(r#"{{"blockdevices":[{roots}]}}"#)).is_err());
    }
}
