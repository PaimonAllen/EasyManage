//! Separate platform key; proxy credentials never share or expose SSH authorization.
use anyhow::{Result, bail};
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct Vault {
    pub directory: PathBuf,
}
fn failure() -> anyhow::Error {
    anyhow::anyhow!("平台代理配置主密钥不可用；请检查私有目录并保留原密钥")
}

impl Vault {
    pub fn cleanup_stale_workspaces(&self) {
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.len() != 18
                || !name.starts_with("r-")
                || !name[2..].bytes().all(|b| b.is_ascii_hexdigit())
            {
                continue;
            }
            if fs::symlink_metadata(entry.path()).is_ok_and(|m| {
                m.is_dir()
                    && m.uid()
                        == fs::metadata("/proc/self")
                            .map(|p| p.uid())
                            .unwrap_or(u32::MAX)
                    && m.mode() & 0o077 == 0
                    && m.modified()
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age.as_secs() > 900)
            }) {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
    pub fn new() -> Result<Self> {
        let directory = std::env::var_os("EASYMANAGE_PROXY_LAB_STATE_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|p| PathBuf::from(p).join(".easymanage/config/proxy-lab"))
            })
            .unwrap_or_else(|| PathBuf::from("/var/lib/easymanage-server/proxy-lab"));
        if !directory.is_absolute() {
            bail!("代理检测状态目录必须是绝对路径");
        }
        Ok(Self { directory })
    }
    pub fn ensure_directory(&self) -> Result<()> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.directory)
            .map_err(|_| failure())?;
        let m = fs::symlink_metadata(&self.directory).map_err(|_| failure())?;
        if !m.is_dir() || m.uid() != fs::metadata("/proc/self")?.uid() || m.mode() & 0o077 != 0 {
            return Err(failure());
        }
        Ok(())
    }
    fn key(&self, create: bool) -> Result<aead::LessSafeKey> {
        self.ensure_directory()?;
        let path = self.directory.join("master.key");
        if create && !path.try_exists().map_err(|_| failure())? {
            let mut bytes = Zeroizing::new([0_u8; 32]);
            SystemRandom::new()
                .fill(bytes.as_mut())
                .map_err(|_| failure())?;
            let tmp = self.directory.join(format!(".key-{}", Uuid::new_v4()));
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&tmp)
                .map_err(|_| failure())?;
            file.write_all(bytes.as_ref())?;
            file.sync_all()?;
            let published = fs::hard_link(&tmp, &path);
            let _ = fs::remove_file(&tmp);
            if let Err(e) = published {
                if e.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(failure());
                }
            }
        }
        let before = fs::symlink_metadata(&path).map_err(|_| failure())?;
        if !before.is_file()
            || before.nlink() != 1
            || before.mode() & 0o077 != 0
            || before.uid() != fs::metadata("/proc/self")?.uid()
            || before.len() != 32
        {
            return Err(failure());
        }
        let mut f = fs::File::open(&path).map_err(|_| failure())?;
        let after = f.metadata()?;
        if after.ino() != before.ino() || after.dev() != before.dev() {
            return Err(failure());
        }
        let mut bytes = Zeroizing::new(Vec::new());
        Read::by_ref(&mut f).take(33).read_to_end(&mut bytes)?;
        if bytes.len() != 32 {
            return Err(failure());
        }
        Ok(aead::LessSafeKey::new(
            aead::UnboundKey::new(&aead::AES_256_GCM, &bytes).map_err(|_| failure())?,
        ))
    }
    pub fn initialize(&self, may_create: bool) -> Result<()> {
        self.key(may_create).map(|_| ())
    }
    pub fn seal(&self, id: Uuid, revision: i64, plain: &[u8]) -> Result<Vec<u8>> {
        if plain.len() > 64 * 1024 {
            bail!("代理节点定义超过存储范围");
        }
        let key = self.key(false)?;
        let mut nonce = [0_u8; 12];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| failure())?;
        let mut body = plain.to_vec();
        key.seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(binding(id, revision)),
            &mut body,
        )
        .map_err(|_| failure())?;
        let mut out = b"P1".to_vec();
        out.extend(nonce);
        out.extend(body);
        Ok(out)
    }
    pub fn open(&self, id: Uuid, revision: i64, record: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        if record.len() < 30 || record.len() > 64 * 1024 + 30 || &record[..2] != b"P1" {
            return Err(failure());
        }
        let nonce: [u8; 12] = record[2..14].try_into().map_err(|_| failure())?;
        let key = self.key(false)?;
        let mut body = Zeroizing::new(record[14..].to_vec());
        let n = key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(binding(id, revision)),
                &mut body,
            )
            .map_err(|_| failure())?
            .len();
        body.truncate(n);
        Ok(body)
    }
    pub fn workspace(&self) -> Result<PathBuf> {
        self.ensure_directory()?;
        let p = self
            .directory
            .join(format!("r-{}", &Uuid::new_v4().simple().to_string()[..16]));
        fs::DirBuilder::new().mode(0o700).create(&p)?;
        Ok(p)
    }
}
fn binding(id: Uuid, revision: i64) -> Vec<u8> {
    format!("easymanage:proxy-lab:v1:{id}:{revision}").into_bytes()
}
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn ciphertext_is_bound_to_identity_revision_and_existing_private_key() {
        let directory = std::env::temp_dir().join(format!("proxy-vault-{}", Uuid::new_v4()));
        let vault = Vault {
            directory: directory.clone(),
        };
        vault.initialize(true).unwrap();
        let id = Uuid::new_v4();
        let record = vault.seal(id, 1, b"fixture-proxy-secret").unwrap();
        assert!(!record.windows(20).any(|s| s == b"fixture-proxy-secret"));
        assert_eq!(
            vault.open(id, 1, &record).unwrap().as_slice(),
            b"fixture-proxy-secret"
        );
        assert!(vault.open(Uuid::new_v4(), 1, &record).is_err());
        assert!(vault.open(id, 2, &record).is_err());
        let mut corrupt = record.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(vault.open(id, 1, &corrupt).is_err());
        let key = directory.join("master.key");
        fs::set_permissions(&key, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(vault.open(id, 1, &record).is_err());
        fs::remove_file(&key).unwrap();
        assert!(vault.initialize(false).is_err());
        assert!(!key.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
