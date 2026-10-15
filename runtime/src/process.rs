//! Stable process-accounting facade; nonimplemented OS metrics are unavailable.

#[cfg(target_os = "linux")]
pub use crate::platform::linux::process::ProcessSampler;
#[cfg(not(target_os = "linux"))]
pub use crate::platform::unavailable::ProcessSampler;
