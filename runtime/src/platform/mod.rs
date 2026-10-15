//! OS-specific runtime implementation details; public facades stay stable.

#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(any(test, not(target_os = "linux")))]
pub(crate) mod unavailable;
