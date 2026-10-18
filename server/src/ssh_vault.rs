//! Owner-only, authenticated encryption for SSH credentials. Never returned by an API.
use super::*;
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

const MAX_RECORD: usize = 24 * 1024;
const COOKIE_AGE: i64 = 30 * 24 * 3600;

#[derive(Deserialize)]
pub(super) struct Saved {
    pub username: String,
    #[serde(deserialize_with = "deserialize_secret")]
    pub password: Zeroizing<String>,
    #[serde(deserialize_with = "deserialize_secret")]
    pub key: Zeroizing<String>,
    #[serde(deserialize_with = "deserialize_secret")]
    pub passphrase: Zeroizing<String>,
    #[serde(deserialize_with = "deserialize_secret")]
    pub sudo_password: Zeroizing<String>,
    pub enabled: bool,
    browser_hash: String,
    browser_expires: i64,
}

impl Saved {
    pub(super) fn auth(&self) -> ssh_transport::Auth {
        ssh_transport::Auth {
            username: self.username.clone(),
            password: self.password.clone(),
            key: self.key.clone(),
            passphrase: self.passphrase.clone(),
        }
    }
}

fn failure() -> OperationsError {
    OperationsError::ssh(
        "SSH 凭据保险库不可用或校验失败；请检查平台专用主密钥和私有目录，未输出任何凭据",
    )
}

fn directory(registry: &FsPath, create: bool) -> Result<PathBuf, OperationsError> {
    machines::check_parent(registry)?;
    let directory = registry.parent().ok_or_else(failure)?.join("ssh-vault");
    if create {
        match fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(failure()),
        }
    }
    let info = fs::symlink_metadata(&directory).map_err(|_| failure())?;
    if !info.is_dir()
        || info.uid() != fs::metadata("/proc/self").map_err(|_| failure())?.uid()
        || info.mode() & 0o077 != 0
    {
        return Err(failure());
    }
    Ok(directory)
}

fn read_private(path: &FsPath, limit: usize) -> Result<Zeroizing<Vec<u8>>, OperationsError> {
    let info = fs::symlink_metadata(path).map_err(|_| failure())?;
    if !info.is_file()
        || info.nlink() != 1
        || info.mode() & 0o077 != 0
        || info.uid() != fs::metadata("/proc/self").map_err(|_| failure())?.uid()
        || info.len() > limit as u64
    {
        return Err(failure());
    }
    let mut file = fs::File::open(path).map_err(|_| failure())?;
    let opened = file.metadata().map_err(|_| failure())?;
    if opened.dev() != info.dev() || opened.ino() != info.ino() {
        return Err(failure());
    }
    let mut bytes = Zeroizing::new(Vec::new());
    (&mut file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| failure())?;
    if bytes.len() > limit {
        return Err(failure());
    }
    Ok(bytes)
}

fn write_private(path: &FsPath, bytes: &[u8], new_key: bool) -> Result<(), OperationsError> {
    let parent = path.parent().ok_or_else(failure)?;
    let temporary = parent.join(format!(".vault-{}", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|_| failure())?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| failure())?;
        if new_key {
            // Atomic publish without ever replacing an existing master key.
            match fs::hard_link(&temporary, path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(failure()),
            }
        } else {
            fs::rename(&temporary, path).map_err(|_| failure())?;
        }
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|_| failure())?;
        Ok(())
    })();
    let _ = fs::remove_file(&temporary);
    result
}

fn key(directory: &FsPath, create: bool) -> Result<aead::LessSafeKey, OperationsError> {
    let path = directory.join("master.key");
    if create && !path.try_exists().map_err(|_| failure())? {
        // Refuse to generate a replacement key next to existing ciphertext.
        if fs::read_dir(directory)
            .map_err(|_| failure())?
            .filter_map(Result::ok)
            .any(|entry| entry.path().extension().is_some_and(|ext| ext == "enc"))
        {
            return Err(failure());
        }
        let mut bytes = Zeroizing::new([0_u8; 32]);
        SystemRandom::new()
            .fill(bytes.as_mut())
            .map_err(|_| failure())?;
        write_private(&path, bytes.as_ref(), true)?;
    }
    let bytes = read_private(&path, 32)?;
    if bytes.len() != 32 {
        return Err(failure());
    }
    Ok(aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, &bytes).map_err(|_| failure())?,
    ))
}

fn binding(target: &Target) -> Result<Vec<u8>, OperationsError> {
    check_private_file(&target.known_hosts)?;
    let host_keys = fs::read(&target.known_hosts).map_err(|_| failure())?;
    Ok(format!(
        "EasyManage SSH vault v1\n{}\n{}\n{}\n{}\n{}",
        target.node_id,
        target.host,
        target.port,
        target.default_user,
        hex::encode(Sha256::digest(host_keys))
    )
    .into_bytes())
}

fn store(registry: &FsPath, target: &Target, saved: &Saved) -> Result<(), OperationsError> {
    let directory = directory(registry, true)?;
    let key = key(&directory, true)?;
    let mut nonce = [0_u8; 12];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| failure())?;
    // Serialize borrowed strings; plaintext lives only in this zeroizing buffer.
    #[derive(Serialize)]
    struct View<'a> {
        username: &'a str,
        password: &'a str,
        key: &'a str,
        passphrase: &'a str,
        sudo_password: &'a str,
        enabled: bool,
        browser_hash: &'a str,
        browser_expires: i64,
    }
    let view = View {
        username: &saved.username,
        password: &saved.password,
        key: &saved.key,
        passphrase: &saved.passphrase,
        sudo_password: &saved.sudo_password,
        enabled: saved.enabled,
        browser_hash: &saved.browser_hash,
        browser_expires: saved.browser_expires,
    };
    let mut ciphertext = Zeroizing::new(serde_json::to_vec(&view).map_err(|_| failure())?);
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(binding(target)?),
        &mut *ciphertext,
    )
    .map_err(|_| failure())?;
    let mut record = b"EMSV1".to_vec();
    record.extend_from_slice(&nonce);
    record.extend_from_slice(&ciphertext);
    if record.len() > MAX_RECORD {
        return Err(failure());
    }
    write_private(
        &directory.join(format!("{}.enc", target.node_id)),
        &record,
        false,
    )
}

pub(super) fn load(registry: &FsPath, target: &Target) -> Result<Saved, OperationsError> {
    let directory = directory(registry, false)?;
    let mut record = read_private(
        &directory.join(format!("{}.enc", target.node_id)),
        MAX_RECORD,
    )?;
    if record.len() < 33 || &record[..5] != b"EMSV1" {
        return Err(failure());
    }
    let nonce: [u8; 12] = record[5..17].try_into().map_err(|_| failure())?;
    let plaintext = key(&directory, false)?
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(binding(target)?),
            &mut record[17..],
        )
        .map_err(|_| failure())?;
    let saved: Saved = serde_json::from_slice(plaintext).map_err(|_| failure())?;
    if saved.username != target.default_user {
        return Err(failure());
    }
    Ok(saved)
}

pub(super) fn save(
    registry: &FsPath,
    target: &Target,
    auth: &ssh_transport::Auth,
    sudo_password: &str,
    enabled: bool,
) -> Result<String, OperationsError> {
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    save_with_token(registry, target, auth, sudo_password, enabled, &token)?;
    Ok(token)
}

pub(super) fn save_with_token(
    registry: &FsPath,
    target: &Target,
    auth: &ssh_transport::Auth,
    sudo_password: &str,
    enabled: bool,
    token: &str,
) -> Result<(), OperationsError> {
    store(
        registry,
        target,
        &Saved {
            username: auth.username.clone(),
            password: auth.password.clone(),
            key: auth.key.clone(),
            passphrase: auth.passphrase.clone(),
            sudo_password: Zeroizing::new(sudo_password.to_owned()),
            enabled,
            browser_hash: crate::api::hash_token(token),
            browser_expires: chrono::Utc::now().timestamp() + COOKIE_AGE,
        },
    )
}

pub(super) fn set_enabled(
    registry: &FsPath,
    target: &Target,
    enabled: bool,
) -> Result<(), OperationsError> {
    let mut saved = load(registry, target)?;
    saved.enabled = enabled;
    store(registry, target, &saved)
}

pub(super) fn exists(registry: &FsPath, target: &Target) -> bool {
    registry.parent().is_some_and(|parent| {
        parent
            .join("ssh-vault")
            .join(format!("{}.enc", target.node_id))
            .is_file()
    })
}

pub(super) fn authorized(saved: &Saved, target: &Target, headers: &HeaderMap) -> bool {
    browser_token(saved, target, headers).is_some()
}

pub(super) fn browser_token(
    saved: &Saved,
    target: &Target,
    headers: &HeaderMap,
) -> Option<Zeroizing<String>> {
    let name = format!("em_ssh_{}=", target.node_id.simple());
    if saved.browser_expires <= chrono::Utc::now().timestamp() {
        return None;
    }
    headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .into_iter()
        .flat_map(|v| v.split(';'))
        .filter_map(|v| v.trim().strip_prefix(&name))
        .find(|token| {
            token.len() == 64
                && crate::api::constant_time_equal(
                    &crate::api::hash_token(token),
                    &saved.browser_hash,
                )
        })
        .map(|token| Zeroizing::new(token.to_owned()))
}

pub(super) fn cookie(target: &Target, token: &str) -> String {
    cookie_for(target.node_id, token)
}

pub(super) fn cookie_for(id: Uuid, token: &str) -> String {
    format!(
        "em_ssh_{}={token}; Path=/api/v1/machines; Secure; HttpOnly; SameSite=Strict; Max-Age={COOKIE_AGE}",
        id.simple()
    )
}

pub(super) fn forget(registry: &FsPath, target: &Target) -> Result<(), OperationsError> {
    let directory = directory(registry, false)?;
    let path = directory.join(format!("{}.enc", target.node_id));
    read_private(&path, MAX_RECORD)?;
    fs::remove_file(path).map_err(|_| failure())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    struct Fixture {
        directory: PathBuf,
        registry: PathBuf,
        target: Target,
    }
    impl Fixture {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("ssh-vault-fixture-{}", Uuid::new_v4()));
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .unwrap();
            let known = directory.join("known_hosts");
            write_private(&known, b"synthetic host public key", false).unwrap();
            let target: Target = serde_json::from_value(json!({"node_id":Uuid::new_v4(),"hostname":"fixture",
                "host":"10.222.0.9","port":2222,"default_user":"fixture","agent_home":"/home/fixture/.easymanage","known_hosts":known})).unwrap();
            Self {
                registry: directory.join("targets.json"),
                directory,
                target,
            }
        }
        fn auth(&self) -> ssh_transport::Auth {
            ssh_transport::Auth {
                username: "fixture".into(),
                password: Zeroizing::new("synthetic-password-not-production".into()),
                key: Zeroizing::new("synthetic-private-key-not-production".into()),
                passphrase: Zeroizing::new("synthetic-key-phrase".into()),
            }
        }
        fn record(&self) -> PathBuf {
            self.directory
                .join("ssh-vault")
                .join(format!("{}.enc", self.target.node_id))
        }
        fn key(&self) -> PathBuf {
            self.directory.join("ssh-vault/master.key")
        }
        fn save(&self) -> String {
            save(
                &self.registry,
                &self.target,
                &self.auth(),
                "synthetic-sudo-secret",
                true,
            )
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn encrypted_round_trip_uses_random_nonce_and_private_permissions() {
        let fixture = Fixture::new();
        fixture.save();
        let first = fs::read(fixture.record()).unwrap();
        let loaded = load(&fixture.registry, &fixture.target).unwrap();
        assert_eq!(&*loaded.password, "synthetic-password-not-production");
        assert_eq!(&*loaded.key, "synthetic-private-key-not-production");
        assert_eq!(&*loaded.passphrase, "synthetic-key-phrase");
        assert_eq!(&*loaded.sudo_password, "synthetic-sudo-secret");
        for secret in [
            &*loaded.password,
            &*loaded.key,
            &*loaded.passphrase,
            &*loaded.sudo_password,
        ] {
            assert!(
                !first
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes())
            );
        }
        fixture.save();
        assert_ne!(first, fs::read(fixture.record()).unwrap());
        assert_eq!(fs::metadata(fixture.key()).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(fixture.record()).unwrap().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(fixture.key().parent().unwrap())
                .unwrap()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn ciphertext_tampering_wrong_key_and_record_swapping_are_rejected() {
        let fixture = Fixture::new();
        fixture.save();
        let original = fs::read(fixture.record()).unwrap();
        let mut changed = original.clone();
        *changed.last_mut().unwrap() ^= 1;
        write_private(&fixture.record(), &changed, false).unwrap();
        assert!(load(&fixture.registry, &fixture.target).is_err());
        write_private(&fixture.record(), &original, false).unwrap();
        let mut other = fixture.target.clone();
        other.host = "10.222.0.10".parse().unwrap();
        assert!(load(&fixture.registry, &other).is_err());
        other = fixture.target.clone();
        other.default_user = "other".into();
        assert!(load(&fixture.registry, &other).is_err());
        other = fixture.target.clone();
        other.node_id = Uuid::new_v4();
        write_private(
            &fixture
                .record()
                .with_file_name(format!("{}.enc", other.node_id)),
            &original,
            false,
        )
        .unwrap();
        assert!(load(&fixture.registry, &other).is_err());
        write_private(&fixture.key(), &[7; 32], false).unwrap();
        assert!(load(&fixture.registry, &fixture.target).is_err());
    }

    #[test]
    fn missing_master_key_is_not_silently_regenerated_for_existing_records() {
        let fixture = Fixture::new();
        fixture.save();
        fs::remove_file(fixture.key()).unwrap();
        assert!(load(&fixture.registry, &fixture.target).is_err());
        assert!(
            save(
                &fixture.registry,
                &fixture.target,
                &fixture.auth(),
                "",
                true
            )
            .is_err()
        );
        assert!(!fixture.key().exists());
    }

    #[test]
    fn insecure_permissions_and_symbolic_links_fail_closed() {
        let fixture = Fixture::new();
        fixture.save();
        fs::set_permissions(fixture.key(), fs::Permissions::from_mode(0o640)).unwrap();
        assert!(load(&fixture.registry, &fixture.target).is_err());
        fs::set_permissions(fixture.key(), fs::Permissions::from_mode(0o600)).unwrap();
        let original = fixture.record().with_extension("original");
        fs::rename(fixture.record(), &original).unwrap();
        symlink(&original, fixture.record()).unwrap();
        assert!(load(&fixture.registry, &fixture.target).is_err());
    }

    #[test]
    fn saved_credentials_require_a_scoped_secure_browser_capability() {
        let fixture = Fixture::new();
        let token = fixture.save();
        let saved = load(&fixture.registry, &fixture.target).unwrap();
        assert!(!authorized(&saved, &fixture.target, &HeaderMap::new()));
        let mut headers = HeaderMap::new();
        headers.insert(
            "cookie",
            format!("em_ssh_{}={token}", fixture.target.node_id.simple())
                .parse()
                .unwrap(),
        );
        assert!(authorized(&saved, &fixture.target, &headers));
        let mut other = fixture.target.clone();
        other.node_id = Uuid::new_v4();
        assert!(!authorized(&saved, &other, &headers));
        let mut expired = saved;
        expired.browser_expires = 0;
        assert!(!authorized(&expired, &fixture.target, &headers));
        let cookie = cookie(&fixture.target, &token);
        for attribute in ["Secure", "HttpOnly", "SameSite=Strict", "Max-Age=2592000"] {
            assert!(cookie.contains(attribute));
        }
    }

    #[test]
    fn pause_and_forget_preserve_host_identity_but_disable_automatic_connections() {
        let fixture = Fixture::new();
        fixture.save();
        set_enabled(&fixture.registry, &fixture.target, false).unwrap();
        assert!(!load(&fixture.registry, &fixture.target).unwrap().enabled);
        forget(&fixture.registry, &fixture.target).unwrap();
        assert!(!exists(&fixture.registry, &fixture.target));
        assert!(fixture.target.known_hosts.exists() && fixture.key().exists());
    }
}
