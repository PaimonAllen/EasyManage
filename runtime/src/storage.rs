//! Bounded metadata-only accounting of files owned by EasyManage.
//! Never descends into symlinks or scans general host directories.
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_FILES: usize = 10_000;
const MAX_DEPTH: usize = 16;

pub fn executable_bytes() -> Option<u64> {
    // Preserve Linux's running-inode behavior, including replaced/deleted binaries.
    #[cfg(target_os = "linux")]
    let metadata = fs::metadata("/proc/self/exe").ok();
    #[cfg(not(target_os = "linux"))]
    let metadata = std::env::current_exe()
        .ok()
        .and_then(|path| fs::metadata(path).ok());
    metadata
        .filter(|item| item.is_file())
        .map(|item| item.len())
}

pub fn sqlite_state_bytes(path: &Path) -> Option<u64> {
    let main = fs::metadata(path).ok().filter(|item| item.is_file())?.len();
    ["-wal", "-shm"]
        .into_iter()
        .try_fold(main, |total, suffix| {
            let mut name: OsString = path.as_os_str().to_os_string();
            name.push(suffix);
            match fs::metadata(PathBuf::from(name)) {
                Ok(item) if item.is_file() => total.checked_add(item.len()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(total),
                _ => None,
            }
        })
}

pub fn bounded_tree_bytes(root: &Path) -> Option<u64> {
    let mut total = 0_u64;
    let mut count = 0_usize;
    let mut pending = vec![(root.to_path_buf(), 0_usize)];
    while let Some((path, depth)) = pending.pop() {
        let metadata = fs::symlink_metadata(&path).ok()?;
        if metadata.file_type().is_symlink() {
            return None;
        }
        count = count.checked_add(1)?;
        if count > MAX_FILES {
            return None;
        }
        if metadata.is_file() {
            total = total.checked_add(metadata.len())?;
        } else if metadata.is_dir() {
            if depth >= MAX_DEPTH {
                return None;
            }
            for entry in fs::read_dir(path).ok()? {
                pending.push((entry.ok()?.path(), depth + 1));
            }
        } else {
            return None;
        }
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::{bounded_tree_bytes, sqlite_state_bytes};
    use std::fs;

    #[test]
    fn account_only_bounded_regular_files() {
        let root =
            std::env::temp_dir().join(format!("easymanage-storage-test-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("state.db"), b"main").unwrap();
        fs::write(root.join("state.db-wal"), b"wal").unwrap();
        fs::create_dir(root.join("web")).unwrap();
        fs::write(root.join("web/index.html"), b"index").unwrap();
        assert_eq!(sqlite_state_bytes(&root.join("state.db")), Some(7));
        assert_eq!(bounded_tree_bytes(&root.join("web")), Some(5));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/", root.join("web/elsewhere")).unwrap();
            assert_eq!(bounded_tree_bytes(&root.join("web")), None);
        }
        fs::remove_dir_all(root).unwrap();
    }
}
