use anyhow::{Context, Result, bail};
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

const SYSTEM_ENV_FILE: &str = "/etc/easymanage/server.env";

#[derive(Clone)]
pub struct ServerConfig {
    pub bind_address: SocketAddr,
    pub database_url: String,
    pub database_max_connections: u32,
    pub enrollment_token: String,
    pub tls_certificate: PathBuf,
    pub tls_private_key: PathBuf,
    pub web_root: PathBuf,
    pub report_interval_seconds: u64,
    pub online_window_seconds: i64,
    pub metrics_retention_days: i32,
}

impl ServerConfig {
    pub fn load() -> Result<Self> {
        if let Some(path) = env::var_os("EASYMANAGE_ENV_FILE") {
            dotenvy::from_path(PathBuf::from(path)).context("cannot read EASYMANAGE_ENV_FILE")?;
        } else {
            let user_config = env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
                .map(|dir| dir.join("easymanage/server.env"));
            if let Some(path) = user_config.filter(|path| path.is_file()) {
                load_environment_file(&path)?;
            } else if Path::new(SYSTEM_ENV_FILE).try_exists().unwrap_or(false) {
                load_environment_file(Path::new(SYSTEM_ENV_FILE))?;
            }
        }
        let config = Self {
            bind_address: env_value("EASYMANAGE_BIND", "0.0.0.0:8443")?
                .parse()
                .context("EASYMANAGE_BIND must be a socket address")?,
            database_url: database_url()?,
            database_max_connections: env_value("EASYMANAGE_DATABASE_MAX_CONNECTIONS", "10")?
                .parse()
                .context("EASYMANAGE_DATABASE_MAX_CONNECTIONS must be an integer")?,
            enrollment_token: required_secret("EASYMANAGE_ENROLLMENT_TOKEN")?,
            tls_certificate: env_value(
                "EASYMANAGE_TLS_CERTIFICATE",
                "/etc/easymanage/tls/server.crt",
            )?
            .into(),
            tls_private_key: env_value(
                "EASYMANAGE_TLS_PRIVATE_KEY",
                "/etc/easymanage/tls/server.key",
            )?
            .into(),
            web_root: env_value("EASYMANAGE_WEB_ROOT", "/opt/easymanage/web")?.into(),
            report_interval_seconds: env_value("EASYMANAGE_REPORT_INTERVAL_SECONDS", "10")?
                .parse()
                .context("EASYMANAGE_REPORT_INTERVAL_SECONDS must be an integer")?,
            online_window_seconds: env_value("EASYMANAGE_ONLINE_WINDOW_SECONDS", "45")?
                .parse()
                .context("EASYMANAGE_ONLINE_WINDOW_SECONDS must be an integer")?,
            metrics_retention_days: env_value("EASYMANAGE_METRICS_RETENTION_DAYS", "30")?
                .parse()
                .context("EASYMANAGE_METRICS_RETENTION_DAYS must be an integer")?,
        };

        if !config.tls_certificate.is_file() {
            bail!(
                "TLS certificate does not exist: {}",
                config.tls_certificate.display()
            );
        }
        if !config.tls_private_key.is_file() {
            bail!(
                "TLS private key does not exist: {}",
                config.tls_private_key.display()
            );
        }
        if config.report_interval_seconds < 5 {
            bail!("EASYMANAGE_REPORT_INTERVAL_SECONDS must be at least 5");
        }
        if config.metrics_retention_days < 1 {
            bail!("EASYMANAGE_METRICS_RETENTION_DAYS must be at least 1");
        }
        if !(1..=64).contains(&config.database_max_connections) {
            bail!("EASYMANAGE_DATABASE_MAX_CONNECTIONS must be between 1 and 64");
        }
        Ok(config)
    }
}

fn database_url() -> Result<String> {
    database_url_from(|key| env::var(key).ok())
}

// Keep configuration portable without interpolating passwords into SQL or URLs.
// A complete URL takes priority so old installations keep their exact settings.
fn database_url_from(get: impl Fn(&str) -> Option<String>) -> Result<String> {
    if let Some(value) = get("DATABASE_URL") {
        if !value.starts_with("postgres://") && !value.starts_with("postgresql://") {
            bail!("DATABASE_URL must use postgres:// or postgresql://");
        }
        value
            .parse::<sqlx::postgres::PgConnectOptions>()
            .map_err(|_| {
                anyhow::anyhow!("DATABASE_URL is not a valid PostgreSQL connection configuration")
            })?;
        return Ok(value);
    }
    let value = |key: &str, default: &str| -> Result<String> {
        let value = get(key).unwrap_or_else(|| default.to_owned());
        if value.trim().is_empty() {
            bail!("{key} cannot be empty");
        }
        Ok(value)
    };
    let host = value("PGHOST", "127.0.0.1")?;
    let port = value("PGPORT", "5432")?
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .context("PGPORT must be between 1 and 65535")?;
    let user = value("PGUSER", "easymanage")?;
    let database = value("PGDATABASE", "easymanage")?;
    let sslmode = value("PGSSLMODE", "prefer")?;
    if !matches!(
        sslmode.as_str(),
        "disable" | "allow" | "prefer" | "require" | "verify-ca" | "verify-full"
    ) {
        bail!("PGSSLMODE is not a supported PostgreSQL SSL mode");
    }
    let mut url = reqwest::Url::parse("postgresql://localhost").expect("fixed PostgreSQL URL");
    // Connection parameters handle socket paths, IPv6 and special characters
    // consistently, rather than hand-assembling a user:password authority.
    {
        let mut params = url.query_pairs_mut();
        params
            .append_pair("host", &host)
            .append_pair("port", &port.to_string())
            .append_pair("user", &user)
            .append_pair("dbname", &database)
            .append_pair("sslmode", &sslmode);
        if let Some(password) = get("PGPASSWORD") {
            params.append_pair("password", &password);
        }
        if let Some(certificate) = get("PGSSLROOTCERT") {
            params.append_pair("sslrootcert", &certificate);
        }
    }
    url.as_str()
        .parse::<sqlx::postgres::PgConnectOptions>()
        .map_err(|_| anyhow::anyhow!("PostgreSQL PG* connection settings are invalid"))?;
    Ok(url.into())
}

fn load_environment_file(path: &Path) -> Result<()> {
    if path.exists() {
        dotenvy::from_path(path)
            .with_context(|| format!("failed to load environment file {}", path.display()))?;
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
                bail!("{name} cannot be empty")
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

#[cfg(test)]
mod tests {
    use super::database_url_from;
    use sqlx::postgres::PgConnectOptions;

    fn settings<'a>(entries: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            entries
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn existing_url_has_priority_and_is_not_rewritten() {
        let original = "postgresql://user:password@localhost:5433/custom?sslmode=disable";
        assert_eq!(
            database_url_from(settings(&[
                ("DATABASE_URL", original),
                ("PGPORT", "invalid")
            ]))
            .unwrap(),
            original
        );
    }

    #[test]
    fn portable_parameters_support_remote_ports_and_password_punctuation() {
        let url = database_url_from(settings(&[
            ("PGHOST", "db.example.internal"),
            ("PGPORT", "15432"),
            ("PGUSER", "operator"),
            ("PGDATABASE", "monitoring"),
            ("PGPASSWORD", "p@ss:/?# &+=中"),
            ("PGSSLMODE", "require"),
        ]))
        .unwrap();
        let options: PgConnectOptions = url.parse().unwrap();
        assert_eq!(options.get_host(), "db.example.internal");
        assert_eq!(options.get_port(), 15432);
        assert_eq!(options.get_username(), "operator");
        assert_eq!(options.get_database(), Some("monitoring"));
        let parsed = reqwest::Url::parse(&url).unwrap();
        assert_eq!(
            parsed
                .query_pairs()
                .find(|(key, _)| key == "password")
                .unwrap()
                .1,
            "p@ss:/?# &+=中"
        );
    }

    #[test]
    fn defaults_do_not_assume_production_port_or_user() {
        let url = database_url_from(|_| None).unwrap();
        let options: PgConnectOptions = url.parse().unwrap();
        assert_eq!(options.get_port(), 5432);
        assert_eq!(options.get_username(), "easymanage");
    }

    #[test]
    fn invalid_settings_do_not_echo_secrets() {
        for entries in [
            vec![("PGPORT", "0")],
            vec![("PGPORT", "65536")],
            vec![("PGSSLMODE", "invalid")],
            vec![("DATABASE_URL", "mysql://secret:private@localhost/db")],
        ] {
            let error = database_url_from(settings(&entries))
                .unwrap_err()
                .to_string();
            assert!(!error.contains("secret:private"));
        }
    }
}
