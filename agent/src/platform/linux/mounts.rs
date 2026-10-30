//! Mount identities and network mounts from procfs only. Never stat an NFS path
//! or enable sysinfo's synchronous network-filesystem capacity probes. Network
//! capacity is handled separately by the hourly isolated sampler.
use easymanage_protocol::{DiskMetric, MountIdentity};
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read},
};

const LIMIT: u64 = 1024 * 1024;

#[derive(Debug)]
pub(super) struct Mount {
    path: String,
    filesystem: String,
    identity: MountIdentity,
}

pub(super) fn is_network_filesystem(filesystem: &str) -> bool {
    matches!(
        filesystem,
        "nfs"
            | "nfs4"
            | "cifs"
            | "smb3"
            | "fuse.sshfs"
            | "sshfs"
            | "9p"
            | "ceph"
            | "glusterfs"
            | "fuse.glusterfs"
    )
}

fn decode(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

fn public_source(value: &str) -> String {
    // Never transmit credentials embedded in a remote source; mount options
    // (which may contain passwords) are deliberately never retained at all.
    if let Some(unc) = value.strip_prefix("//") {
        let (authority, path) = unc.split_once('/').unwrap_or((unc, ""));
        return format!(
            "//{}/{}",
            authority.rsplit('@').next().unwrap_or(authority),
            path
        );
    }
    if let Some((authority, path)) = value.split_once(":/") {
        return format!(
            "{}:/{}",
            authority.rsplit('@').next().unwrap_or(authority),
            path
        );
    }
    value.to_owned()
}

pub(super) fn parse(contents: &str) -> io::Result<Vec<Mount>> {
    if contents.len() > LIMIT as usize {
        return Err(io::Error::other("mount inventory too large"));
    }
    let mut entries = HashMap::new();
    for line in contents.lines() {
        let (left, right) = line
            .split_once(" - ")
            .ok_or_else(|| io::Error::other("invalid mount inventory"))?;
        let fields: Vec<_> = left.split_whitespace().collect();
        let rest: Vec<_> = right.split_whitespace().collect();
        if fields.len() < 6 || rest.len() < 3 || !fields[4].starts_with('/') {
            return Err(io::Error::other("invalid mount inventory"));
        }
        let mount_id = fields[0].parse::<u64>().map_err(io::Error::other)?;
        let path = decode(fields[4]);
        let network = is_network_filesystem(rest[0]);
        let identity = MountIdentity {
            device_id: fields[2].to_owned(),
            root: decode(fields[3]),
            source: public_source(&decode(rest[1])),
            network,
            read_only: fields[5].split(',').any(|option| option == "ro"),
            capacity_known: false,
            capacity_probe: None,
        };
        // For stacked mounts at one path, retain the later mount's identity.
        if entries
            .get(&path)
            .is_none_or(|(old_id, _): &(u64, Mount)| mount_id > *old_id)
        {
            entries.insert(
                path.clone(),
                (
                    mount_id,
                    Mount {
                        path,
                        filesystem: rest[0].into(),
                        identity,
                    },
                ),
            );
        }
    }
    let mut mounts: Vec<_> = entries.into_values().map(|(_, mount)| mount).collect();
    mounts.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(mounts)
}

pub(super) fn read() -> io::Result<Vec<Mount>> {
    let mut contents = String::new();
    File::open("/proc/self/mountinfo")?
        .take(LIMIT + 1)
        .read_to_string(&mut contents)?;
    parse(&contents)
}

pub(super) fn enrich(disks: &mut Vec<DiskMetric>, mounts: Vec<Mount>) {
    let mut by_path: HashMap<_, _> = mounts
        .into_iter()
        .map(|mount| (mount.path.clone(), mount))
        .collect();
    for disk in disks.iter_mut() {
        if let Some(mut mount) = by_path.remove(&disk.mount_point) {
            if mount.identity.network {
                disk.name.clone_from(&mount.identity.source);
            }
            mount.identity.capacity_known =
                disk.total_bytes > 0 && disk.available_bytes <= disk.total_bytes;
            disk.mount = Some(mount.identity);
        }
    }
    for mount in by_path
        .into_values()
        .filter(|mount| mount.identity.network || mount.filesystem == "autofs")
    {
        disks.push(DiskMetric {
            name: mount.identity.source.clone(),
            mount_point: mount.path,
            file_system: mount.filesystem,
            total_bytes: 0,
            available_bytes: 0,
            read_bytes_per_second: None,
            written_bytes_per_second: None,
            mount: Some(mount.identity),
        });
    }
    disks.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn network_mounts_and_bind_roots_are_metadata_only() {
        let text = "1 0 8:1 / /data rw - ext4 /dev/sda1 rw\n2 1 8:1 /users/tzr/data /home/tzr/bigdata rw - ext4 /dev/sda1 rw\n3 1 0:41 / /nfs rw - nfs4 10.0.0.1:/export rw,secret=FIXTURE_SECRET\n4 1 0:42 / /auto rw - autofs systemd-1 rw\n5 1 0:43 / /space\\040name ro - cifs //user:FIXTURE_SECRET@host/share rw,password=FIXTURE_SECRET\n";
        let mut disks = vec![
            DiskMetric {
                name: "/dev/sda1".into(),
                mount_point: "/data".into(),
                file_system: "ext4".into(),
                total_bytes: 100,
                available_bytes: 70,
                read_bytes_per_second: None,
                written_bytes_per_second: None,
                mount: None,
            },
            DiskMetric {
                name: "/dev/sda1".into(),
                mount_point: "/home/tzr/bigdata".into(),
                file_system: "ext4".into(),
                total_bytes: 100,
                available_bytes: 70,
                read_bytes_per_second: None,
                written_bytes_per_second: None,
                mount: None,
            },
        ];
        enrich(&mut disks, parse(text).unwrap());
        assert_eq!(disks.len(), 5);
        let nfs = disks.iter().find(|d| d.mount_point == "/nfs").unwrap();
        assert!(nfs.mount.as_ref().unwrap().network);
        assert!(!nfs.mount.as_ref().unwrap().capacity_known);
        assert!(nfs.read_bytes_per_second.is_none());
        let data = disks
            .iter()
            .find(|d| d.mount_point == "/data")
            .unwrap()
            .mount
            .as_ref()
            .unwrap();
        let bind = disks
            .iter()
            .find(|d| d.mount_point == "/home/tzr/bigdata")
            .unwrap()
            .mount
            .as_ref()
            .unwrap();
        assert_eq!(data.device_id, bind.device_id);
        assert_eq!(bind.root, "/users/tzr/data");
        assert!(data.capacity_known);
        let smb = disks
            .iter()
            .find(|d| d.mount_point == "/space name")
            .unwrap();
        assert!(smb.mount.as_ref().unwrap().read_only);
        let serialized = serde_json::to_string(&disks).unwrap();
        assert!(!serialized.contains("FIXTURE_SECRET"));
    }
    #[test]
    fn malformed_and_oversized_inventory_fails_closed() {
        assert!(parse("bad").is_err());
        assert!(parse(&"x".repeat(LIMIT as usize + 1)).is_err());
        let mounts = parse(
            "1 0 8:1 / /data rw - ext4 /dev/sda1 rw\n2 0 8:2 / /data rw - ext4 /dev/sdb1 rw\n",
        )
        .unwrap();
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].identity.device_id, "8:2");
    }
}
