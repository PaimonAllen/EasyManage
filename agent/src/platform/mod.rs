//! Native collector selection. Shared orchestration depends only on the contract.
//! Only implemented and verified platforms are registered here.

pub(crate) mod common;
mod contract;
#[cfg(target_os = "linux")]
mod linux;

pub(crate) use contract::PlatformCollector;
#[cfg(target_os = "linux")]
pub(crate) use linux::NativeCollector;
#[cfg(target_os = "linux")]
pub(crate) use linux::paths;

/// Keep orchestration dependent on the contract, not Linux's inherent methods.
#[cfg(target_os = "linux")]
pub(crate) async fn initialize(
    state_database: &std::path::Path,
    store: &crate::store::Store,
) -> anyhow::Result<NativeCollector> {
    <linux::NativeCollector as PlatformCollector>::initialize(state_database, store).await
}
