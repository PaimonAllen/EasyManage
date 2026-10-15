use anyhow::{Context, Result, bail};
use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::platform::paths::{SYSTEM_ENV_FILE, default_state_path, user_config_path};

#[derive(Clone)]
pub struct AgentConfig {
    pub server_url: String,
    pub ca_certificate: PathBuf,
    pub state_database: PathBuf,
    pub enrollment_token: String,
    pub report_interval: Duration,
    pub request_timeout: Duration,
}

impl AgentConfig {
    pub fn load() -> Result<Self> {
        if let Some(path) = env::var_os("EASYMANAGE_ENV_FILE") {
            if path.is_empty() {
                bail!("EASYMANAGE_ENV_FILE cannot be empty");
            }
            dotenvy::from_path(Path::new(&path))
                .context("failed to load explicit EASYMANAGE_ENV_FILE")?;
        } else {
            if let Some(path) = user_config_path() {
                load_implicit_environment_file(&path)?;
            }
            load_implicit_environment_file(Path::new(SYSTEM_ENV_FILE))?;
        }

        let server_url = required_env("EASYMANAGE_SERVER_URL")?
            .trim_end_matches('/')
            .to_owned();
        if !server_url.starts_with("https://") {
            bail!("EASYMANAGE_SERVER_URL must use https://");
        }

        let ca_certificate = PathBuf::from(required_env("EASYMANAGE_CA_CERTIFICATE")?);
        if !ca_certificate.is_file() {
            bail!(
                "CA certificate does not exist: {}",
                ca_certificate.display()
            );
        }

        let report_interval_seconds = env_value("EASYMANAGE_REPORT_INTERVAL_SECONDS", "10")?
            .parse::<u64>()
            .context("EASYMANAGE_REPORT_INTERVAL_SECONDS must be an integer")?;
        if !(5..=30).contains(&report_interval_seconds) {
            bail!("EASYMANAGE_REPORT_INTERVAL_SECONDS must be between 5 and 30");
        }

        let request_timeout_seconds = env_value("EASYMANAGE_REQUEST_TIMEOUT_SECONDS", "15")?
            .parse::<u64>()
            .context("EASYMANAGE_REQUEST_TIMEOUT_SECONDS must be an integer")?;
        if !(1..=120).contains(&request_timeout_seconds) {
            bail!("EASYMANAGE_REQUEST_TIMEOUT_SECONDS must be between 1 and 120");
        }

        Ok(Self {
            server_url,
            ca_certificate,
            state_database: match env::var_os("EASYMANAGE_AGENT_STATE_DB") {
                Some(path) if !path.is_empty() => PathBuf::from(path),
                Some(_) => bail!("EASYMANAGE_AGENT_STATE_DB cannot be empty"),
                None => default_state_path()?,
            },
            enrollment_token: required_secret("EASYMANAGE_ENROLLMENT_TOKEN")?,
            report_interval: Duration::from_secs(report_interval_seconds),
            request_timeout: Duration::from_secs(request_timeout_seconds),
        })
    }
}

fn load_implicit_environment_file(path: &Path) -> Result<()> {
    match std::fs::File::open(path) {
        Ok(file) => {
            dotenvy::from_read(file)
                .with_context(|| format!("failed to load environment file {}", path.display()))?;
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) => {}
        Err(error) => {
            return Err(error).with_context(|| format!("failed to open {}", path.display()));
        }
    }
    Ok(())
}

fn env_value(name: &str, default: &str) -> Result<String> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) => bail!("{name} cannot be empty"),
        Err(env::VarError::NotPresent) => Ok(default.to_owned()),
        Err(error) => Err(error).with_context(|| format!("failed to read {name}")),
    }
}

fn required_env(name: &str) -> Result<String> {
    env::var(name)
        .with_context(|| format!("required environment variable {name} is missing"))
        .and_then(|value| {
            if value.trim().is_empty() {
                bail!("{name} cannot be empty");
            }
            Ok(value)
        })
}

fn required_secret(name: &str) -> Result<String> {
    let value = required_env(name)?;
    if value.len() < 32 {
        bail!("{name} must contain at least 32 characters");
    }
    Ok(value)
}
