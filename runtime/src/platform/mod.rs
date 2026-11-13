//! OS-specific runtime implementation details; public facades stay stable.

#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "windows")]
pub(crate) mod windows;
#[cfg(any(test, not(any(target_os = "linux", target_os = "windows"))))]
pub(crate) mod unavailable;
