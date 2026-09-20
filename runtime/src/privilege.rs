use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fmt::{self, Display, Formatter};
use std::process::Command;

const ADMIN_POLICY_ENV: &str = "EASYMANAGE_ADMIN_POLICY";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AdminPolicy {
    #[default]
    WarnAndContinue,
    Require,
}

impl AdminPolicy {
    #[must_use]
    pub fn from_environment() -> Self {
        parse_policy(env::var_os(ADMIN_POLICY_ENV).as_deref())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PrivilegeError {
    NotAdministrator,
    CheckFailed(String),
}

impl Display for PrivilegeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAdministrator => formatter.write_str("administrator privileges are required"),
            Self::CheckFailed(reason) => {
                write!(formatter, "administrator privilege check failed: {reason}")
            }
        }
    }
}

impl Error for PrivilegeError {}

/// Checks startup privileges using the current policy.
///
/// The default `warn` policy reports the privilege error and allows startup to continue.
/// Set `EASYMANAGE_ADMIN_POLICY=require` to turn the same condition into a startup failure.
///
/// # Errors
///
/// Returns the detected privilege error when the policy is `require`.
pub fn ensure_administrator(component: &str) -> Result<(), PrivilegeError> {
    match check_administrator() {
        Ok(()) => {
            eprintln!("[INFO] EasyManage {component} is running with administrator privileges");
            Ok(())
        }
        Err(error) if AdminPolicy::from_environment() == AdminPolicy::WarnAndContinue => {
            eprintln!(
                "[ERROR] EasyManage {component}: {error}; continuing in degraded mode. \
                 Set {ADMIN_POLICY_ENV}=require to reject non-administrator startup in the future."
            );
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn check_administrator() -> Result<(), PrivilegeError> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .map_err(|error| PrivilegeError::CheckFailed(error.to_string()))?;

    if !output.status.success() {
        return Err(PrivilegeError::CheckFailed(format!(
            "`id -u` exited with {}",
            output.status
        )));
    }

    let user_id = String::from_utf8(output.stdout)
        .map_err(|error| PrivilegeError::CheckFailed(error.to_string()))?;

    if user_id.trim() == "0" {
        Ok(())
    } else {
        Err(PrivilegeError::NotAdministrator)
    }
}

#[cfg(windows)]
fn check_administrator() -> Result<(), PrivilegeError> {
    let command = concat!(
        "([Security.Principal.WindowsPrincipal] ",
        "[Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(",
        "[Security.Principal.WindowsBuiltInRole]::Administrator)"
    );
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", command])
        .output()
        .map_err(|error| PrivilegeError::CheckFailed(error.to_string()))?;

    if !output.status.success() {
        return Err(PrivilegeError::CheckFailed(format!(
            "PowerShell exited with {}",
            output.status
        )));
    }

    let is_administrator = String::from_utf8(output.stdout)
        .map_err(|error| PrivilegeError::CheckFailed(error.to_string()))?;

    if is_administrator.trim().eq_ignore_ascii_case("true") {
        Ok(())
    } else {
        Err(PrivilegeError::NotAdministrator)
    }
}

#[cfg(not(any(unix, windows)))]
fn check_administrator() -> Result<(), PrivilegeError> {
    Err(PrivilegeError::CheckFailed(
        "unsupported operating system".to_owned(),
    ))
}

fn parse_policy(value: Option<&OsStr>) -> AdminPolicy {
    value
        .and_then(OsStr::to_str)
        .filter(|value| value.eq_ignore_ascii_case("require"))
        .map_or(AdminPolicy::WarnAndContinue, |_| AdminPolicy::Require)
}

#[cfg(test)]
mod tests {
    use super::{AdminPolicy, OsStr, parse_policy};

    #[test]
    fn warn_and_continue_is_the_default_policy() {
        assert_eq!(parse_policy(None), AdminPolicy::WarnAndContinue);
        assert_eq!(
            parse_policy(Some(OsStr::new("warn"))),
            AdminPolicy::WarnAndContinue
        );
    }

    #[test]
    fn require_policy_is_case_insensitive() {
        assert_eq!(
            parse_policy(Some(OsStr::new("ReQuIrE"))),
            AdminPolicy::Require
        );
    }
}
