//! User deployments live under ~/.easymanage; Full state remains system-owned.
//! Explicit configuration and existing legacy identities must never be discarded.

use anyhow::{Context, Result, bail};
use std::env;
use std::path::PathBuf;

pub(crate) const SYSTEM_ENV_FILE: &str = "/etc/easymanage/agent.env";

pub(crate) fn user_config_path() -> Option<PathBuf> {
    let preferred = user_config_path_from(|key| env::var_os(key).map(PathBuf::from));
    if preferred.as_ref().is_some_and(|path| path.is_file()) { return preferred; }
    // Existing configurations can still specify the original identity database.
    let legacy = env::var_os("HOME").map(PathBuf::from)
        .map(|home| home.join(".local/share/easymanage/config/agent.env"));
    let xdg = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|base| base.join("easymanage/agent.env"));
    legacy.filter(|path| path.is_file()).or_else(|| xdg.filter(|path| path.is_file())).or(preferred)
}

fn user_config_path_from(get: impl Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    get("HOME").map(|home| home.join(".easymanage/config/agent.env")).or_else(|| get("XDG_CONFIG_HOME")
        .filter(|value| !value.as_os_str().is_empty())
        .map(|base| base.join("easymanage/agent.env")))
}

pub(crate) fn default_state_path() -> Result<PathBuf> {
    let uid = super::inventory::effective_uid();
    let preferred = default_state_path_from(uid, |key| {
        env::var_os(key).map(PathBuf::from)
    })?;
    if uid != Some(0) && !preferred.exists() {
        if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
            if [home.join(".local/share/easymanage/state/state.db"), home.join(".local/state/easymanage/agent/state.db")]
                .iter().any(|path| path.is_file()) {
                bail!("legacy Agent identity exists; migrate it to ~/.easymanage or set EASYMANAGE_AGENT_STATE_DB explicitly");
            }
        }
    }
    Ok(preferred)
}

fn default_state_path_from(
    uid: Option<u32>,
    get: impl Fn(&str) -> Option<PathBuf>,
) -> Result<PathBuf> {
    if uid == Some(0) {
        return Ok(PathBuf::from("/var/lib/easymanage-agent/state.db"));
    }
    get("HOME").map(|home| home.join(".easymanage/state/state.db")).or_else(|| get("XDG_STATE_HOME")
        .filter(|value| !value.as_os_str().is_empty())
        .map(|base| base.join("easymanage/agent/state.db")))
        .context("set EASYMANAGE_AGENT_STATE_DB when HOME and XDG_STATE_HOME are unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_state_path_does_not_follow_user_environment() {
        let path =
            default_state_path_from(Some(0), |_| Some(PathBuf::from("/tmp/fixture"))).unwrap();
        assert_eq!(path, PathBuf::from("/var/lib/easymanage-agent/state.db"));
    }

    #[test]
    fn normal_user_paths_use_the_managed_home_directory() {
        let get = |key: &str| match key {
            "XDG_CONFIG_HOME" => Some(PathBuf::from("/tmp/config")),
            "XDG_STATE_HOME" => Some(PathBuf::from("/tmp/state")),
            "HOME" => Some(PathBuf::from("/tmp/home")),
            _ => None,
        };
        assert_eq!(
            user_config_path_from(get),
            Some(PathBuf::from("/tmp/home/.easymanage/config/agent.env"))
        );
        assert_eq!(
            default_state_path_from(Some(1001), get).unwrap(),
            PathBuf::from("/tmp/home/.easymanage/state/state.db")
        );
        let home_only = |key: &str| (key == "HOME").then(|| PathBuf::from("/tmp/home"));
        assert_eq!(
            user_config_path_from(home_only),
            Some(PathBuf::from("/tmp/home/.easymanage/config/agent.env"))
        );
        assert_eq!(
            default_state_path_from(Some(1001), home_only).unwrap(),
            PathBuf::from("/tmp/home/.easymanage/state/state.db")
        );
    }

    #[test]
    fn missing_environment_never_falls_back_to_root_state() {
        assert!(default_state_path_from(None, |_| None).is_err());
        assert_eq!(user_config_path_from(|_| None), None);
    }
}
