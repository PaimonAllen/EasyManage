use crate::store::Store;
use anyhow::Result;
use easymanage_protocol::{CollectionPolicy, LiveMetrics, MetricsSnapshot, NodeInventory};
use std::path::Path;

/// Statically dispatched platform boundary: no trait objects or extra workers.
/// Each backend owns OS probes/cache semantics and reports missing data honestly.
pub(crate) trait PlatformCollector: Sized {
    async fn initialize(state_database: &Path, store: &Store) -> Result<Self>;
    fn apply_policy(&mut self, policy: CollectionPolicy);
    fn inventory(&mut self) -> Result<NodeInventory>;
    fn collect_fast(&mut self) -> LiveMetrics;
    async fn collect(&mut self) -> MetricsSnapshot;
}
