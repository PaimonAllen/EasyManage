//! Stable process-accounting facade; nonimplemented OS metrics are unavailable.

#[cfg(target_os = "linux")]
pub use crate::platform::linux::process::ProcessSampler;
#[cfg(target_os = "windows")]
pub use crate::platform::windows::process::ProcessSampler;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub use crate::platform::unavailable::ProcessSampler;
