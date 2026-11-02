//! Existing Linux backend. Moving these modules does not change probe commands,
//! state paths, sampling periods, privileges or report fields.

mod ai_tools;
mod block_devices;
mod collector;
mod cpu_frequency;
mod hardware;
pub(super) mod inventory;
mod mounts;
mod network_capacity;
mod network_counters;
mod overlay_networks;
pub(crate) mod paths;
mod permissions;
mod programming_tools;
mod security;
mod software;
mod temperature;

pub(crate) use collector::Collector as NativeCollector;

impl super::PlatformCollector for NativeCollector {
    async fn initialize(
        state_database: &std::path::Path,
        store: &crate::store::Store,
    ) -> anyhow::Result<Self> {
        Ok(Self::initialize(state_database, store).await)
    }

    fn apply_policy(&mut self, policy: easymanage_protocol::CollectionPolicy) {
        Self::apply_policy(self, policy);
    }

    fn inventory(&mut self) -> anyhow::Result<easymanage_protocol::NodeInventory> {
        Self::inventory(self)
    }

    fn collect_fast(&mut self) -> easymanage_protocol::LiveMetrics {
        Self::collect_fast(self)
    }

    async fn collect(&mut self) -> easymanage_protocol::MetricsSnapshot {
        Self::collect(self).await
    }
}
