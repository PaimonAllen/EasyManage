//! Linux state/config layout, including legacy user mode. Keep paths unchanged
//! so moving source files can never create a new production node identity.

use anyhow::{Context, Result};
use std::env;
use std::path::PathBuf;

pub(crate) const SYSTEM_ENV_FILE: &str = "/etc/easymanage/agent.env";

pub(crate) fn user_config_path() -> Option<PathBuf> {
    user_config_path_from(|key| env::var_os(key).map(PathBuf::from))
}

fn user_config_path_from(get: impl Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    get("XDG_CONFIG_HOME")
        .filter(|value| !value.as_os_str().is_empty())
        .or_else(|| get("HOME").map(|home| home.join(".config")))
        .map(|base| base.join("easymanage/agent.env"))
}

pub(crate) fn default_state_path() -> Result<PathBuf> {
    default_state_path_from(super::inventory::effective_uid(), |key| {
        env::var_os(key).map(PathBuf::from)
    })
}

fn default_state_path_from(
    uid: Option<u32>,
    get: impl Fn(&str) -> Option<PathBuf>,
) -> Result<PathBuf> {
    if uid == Some(0) {
        return Ok(PathBuf::from("/var/lib/easymanage-agent/state.db"));
    }
    get("XDG_STATE_HOME")
        .filter(|value| !value.as_os_str().is_empty())
        .or_else(|| get("HOME").map(|home| home.join(".local/state")))
        .map(|base| base.join("easymanage/agent/state.db"))
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
    fn normal_user_paths_preserve_xdg_and_home_precedence() {
        let get = |key: &str| match key {
            "XDG_CONFIG_HOME" => Some(PathBuf::from("/tmp/config")),
            "XDG_STATE_HOME" => Some(PathBuf::from("/tmp/state")),
            "HOME" => Some(PathBuf::from("/tmp/home")),
            _ => None,
        };
        assert_eq!(
            user_config_path_from(get),
            Some(PathBuf::from("/tmp/config/easymanage/agent.env"))
        );
        assert_eq!(
            default_state_path_from(Some(1001), get).unwrap(),
            PathBuf::from("/tmp/state/easymanage/agent/state.db")
        );
        let home_only = |key: &str| (key == "HOME").then(|| PathBuf::from("/tmp/home"));
        assert_eq!(
            user_config_path_from(home_only),
            Some(PathBuf::from("/tmp/home/.config/easymanage/agent.env"))
        );
        assert_eq!(
            default_state_path_from(Some(1001), home_only).unwrap(),
            PathBuf::from("/tmp/home/.local/state/easymanage/agent/state.db")
        );
    }

    #[test]
    fn missing_environment_never_falls_back_to_root_state() {
        assert!(default_state_path_from(None, |_| None).is_err());
        assert_eq!(user_config_path_from(|_| None), None);
    }
}
