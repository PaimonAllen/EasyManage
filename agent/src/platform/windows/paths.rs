use anyhow::{Context, Result};
use std::{env, path::PathBuf};
pub(crate) fn system_config_path() -> Option<PathBuf> {
    env::var_os("ProgramData").map(|p| PathBuf::from(p).join("EasyManage/config/agent.env"))
}
pub(crate) fn user_config_path() -> Option<PathBuf> {
    env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("EasyManage/config/agent.env"))
}
pub(crate) fn default_state_path() -> Result<PathBuf> {
    // Full services always configure an explicit state path. Never move an identity
    // merely because an interactive process has an elevated token.
    env::var_os("LOCALAPPDATA")
        .map(|p| PathBuf::from(p).join("EasyManage/state/state.db"))
        .context("set EASYMANAGE_AGENT_STATE_DB when LOCALAPPDATA is unavailable")
}
