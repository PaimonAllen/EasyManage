//! Bounded, content-free application network accounting, not NIC/wire accounting.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[repr(usize)]
pub enum NetworkGroup {
    HttpBrowser,
    HttpAgent,
    HttpService,
    HttpProbe,
    AgentEnroll,
    AgentReport,
    AgentLive,
    AgentControl,
    SshConnect,
    SshExec,
    SshScp,
    SshTunnel,
    TcpProbe,
    WebSocket,
}
impl NetworkGroup {
    pub const ALL: [Self; 14] = [
        Self::HttpBrowser,
        Self::HttpAgent,
        Self::HttpService,
        Self::HttpProbe,
        Self::AgentEnroll,
        Self::AgentReport,
        Self::AgentLive,
        Self::AgentControl,
        Self::SshConnect,
        Self::SshExec,
        Self::SshScp,
        Self::SshTunnel,
        Self::TcpProbe,
        Self::WebSocket,
    ];
    pub fn is_agent(self) -> bool {
        matches!(
            self,
            Self::AgentEnroll | Self::AgentReport | Self::AgentLive | Self::AgentControl
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NetworkGroupMetric {
    pub group: NetworkGroup,
    pub started: u64,
    pub completed: u64,
    pub failed: u64,
    pub in_flight: u64,
    pub open_connections: Option<u64>,
    pub received_bytes: u64,
    pub submitted_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NetworkActivity {
    pub process_id: u32,
    pub started_at: DateTime<Utc>,
    pub collected_at: DateTime<Utc>,
    pub groups: Vec<NetworkGroupMetric>,
}
