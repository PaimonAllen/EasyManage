//! Revisioned, per-node monitoring control. This is not an enrollment identity.
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentControl {
    pub revision: u64,
    pub paused: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentControlRequest {
    pub node_id: Uuid,
    pub applied: AgentControl,
    #[serde(default)]
    pub supports_proxy_collection: bool,
}
#[derive(Deserialize, Serialize)]
pub struct AgentControlResponse {
    pub control: AgentControl,
    pub collection_policy: crate::CollectionPolicy,
}
