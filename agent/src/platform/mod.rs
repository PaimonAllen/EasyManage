//! Native collector selection. Shared orchestration depends only on the contract.
//! Only implemented and verified platforms are registered here.

pub(crate) mod common;
mod contract;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub(crate) use windows::{NativeCollector, diagnostics, paths};

pub(crate) use contract::PlatformCollector;
#[cfg(target_os = "linux")]
pub(crate) use linux::NativeCollector;
#[cfg(target_os = "linux")]
pub(crate) use linux::paths;

/// Keep orchestration dependent on the contract, not Linux's inherent methods.
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) async fn initialize(
    state_database: &std::path::Path,
    store: &crate::store::Store,
) -> anyhow::Result<NativeCollector> {
    <NativeCollector as PlatformCollector>::initialize(state_database, store).await
}
