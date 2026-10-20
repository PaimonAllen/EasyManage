#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 1;
pub const AGENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const ENROLLMENT_TOKEN_HEADER: &str = "x-easymanage-enrollment-token";
pub const AGENT_TOKEN_HEADER: &str = "x-easymanage-agent-token";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NodeInventory {
    pub hostname: String,
    pub os_name: String,
    pub os_version: String,
    pub kernel_version: String,
    pub architecture: String,
    pub cpu_model: String,
    pub cpu_logical_cores: u32,
    pub cpu_physical_cores: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiskMetric {
    pub name: String,
    pub mount_point: String,
    pub file_system: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    #[serde(default)]
    pub read_bytes_per_second: Option<f64>,
    #[serde(default)]
    pub written_bytes_per_second: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BlockDevice {
    pub name: String,
    pub path: Option<String>,
    pub kind: String,
    pub size_bytes: u64,
    pub filesystem: Option<String>,
    pub label: Option<String>,
    pub mount_points: Vec<String>,
    pub model: Option<String>,
    pub transport: Option<String>,
    pub rotational: Option<bool>,
    pub removable: Option<bool>,
    pub read_only: Option<bool>,
    pub partition_table: Option<String>,
    pub partition_label: Option<String>,
    pub start_bytes: Option<u64>,
    #[serde(default)]
    pub children: Vec<BlockDevice>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AgentPrivilege {
    pub mode: String,
    pub username: String,
    pub uid: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UserStatus {
    pub username: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    pub shell: String,
    pub groups: Vec<String>,
    pub sessions: Option<u32>,
    #[serde(default)]
    pub session_sources: Vec<String>,
    #[serde(default)]
    pub session_evidence: Option<UserSessionEvidence>,
    pub process_count: Option<u32>,
    pub locked: Option<bool>,
    #[serde(default)]
    pub account_status: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UserSessionEvidence {
    pub terminal_records: Option<u32>,
    pub logind_sessions: Option<u32>,
    pub ssh_processes: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NetworkInterface {
    pub name: String,
    pub kind: String,
    pub state: String,
    pub addresses: Vec<String>,
    pub mac_address: Option<String>,
    pub mtu: Option<u32>,
    pub vlan_id: Option<u32>,
    pub master: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OverlayMembership {
    pub network_id: String,
    pub name: Option<String>,
    pub state: String,
    pub interface: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OverlayNetworkStatus {
    pub provider: String,
    pub state: String,
    pub detail: String,
    pub networks: Vec<OverlayMembership>,
}

/// Resource usage of this EasyManage process only; unavailable counters stay null.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PlatformProcessMetric {
    pub pid: u32,
    /// CPU time divided by all host CPU time in the sampling interval (0..100).
    pub cpu_percent_of_host: Option<f64>,
    pub resident_memory_bytes: Option<u64>,
    pub disk_read_bytes_per_second: Option<f64>,
    pub disk_written_bytes_per_second: Option<f64>,
}

/// Metadata-only footprint of EasyManage-owned files; absent parts are unknown, not zero.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PlatformStorageMetric {
    pub collected_at: DateTime<Utc>,
    pub binary_bytes: Option<u64>,
    pub state_bytes: Option<u64>,
    pub web_bytes: Option<u64>,
    pub database_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
}

/// Daily hardware inventory. Strings contain public model/capacity information only.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HardwareDetails {
    pub collected_at: DateTime<Utc>,
    pub processors: Vec<String>,
    pub motherboard: Vec<String>,
    pub memory: Vec<String>,
    pub graphics: Vec<String>,
    pub displays: Vec<String>,
    pub disks: Vec<String>,
    pub audio: Vec<String>,
    pub network: Vec<String>,
    pub notes: Vec<String>,
}

impl HardwareDetails {
    pub fn valid(&self) -> bool {
        [&self.processors, &self.motherboard, &self.memory, &self.graphics,
         &self.displays, &self.disks, &self.audio, &self.network, &self.notes]
            .into_iter().all(|items| items.len() <= 128 && items.iter().all(|text| text.len() <= 1024))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RouteInfo {
    pub destination: String,
    pub gateway: Option<String>,
    pub interface: Option<String>,
    pub metric: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProxyService {
    pub name: String,
    pub pid: u32,
    pub username: String,
    pub uid: u32,
    pub listen_addresses: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CollectionIssue {
    pub component: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SoftwarePackage {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServiceStatus {
    pub name: String,
    pub state: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AiToolProcess {
    pub pid: u32,
    pub uid: u32,
    pub username: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AiToolStatus {
    pub name: String,
    pub category: String,
    pub executable_paths: Vec<String>,
    pub processes: Vec<AiToolProcess>,
    pub service_units: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProgrammingToolStatus {
    pub name: String,
    pub category: String,
    pub executable_paths: Vec<String>,
    pub package_version: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostSecurity {
    pub apparmor_enabled: Option<bool>,
    pub aslr_mode: Option<u8>,
    pub ptrace_scope: Option<u8>,
    pub kernel_lockdown: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PermissionCheck {
    pub category: String,
    pub title: String,
    pub path: String,
    pub status: String,
    pub observed: String,
    pub expected: String,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PermissionAudit {
    pub collected_at: DateTime<Utc>,
    pub checks: Vec<PermissionCheck>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NetworkMetric {
    pub interface: String,
    pub received_bytes_per_second: f64,
    pub transmitted_bytes_per_second: f64,
    pub total_received_bytes: u64,
    pub total_transmitted_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GpuMetric {
    pub index: u32,
    pub name: String,
    pub utilization_percent: f32,
    pub memory_total_bytes: u64,
    pub memory_used_bytes: u64,
    pub temperature_celsius: Option<f32>,
    pub power_watts: Option<f32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SsdTemperature {
    pub device: String,
    pub temperature_celsius: f32,
}

/// Only positively identified CPU/solid-state sensors; missing is never zero.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostTemperatures {
    pub collected_at: DateTime<Utc>,
    pub cpu_celsius: Option<f32>,
    pub ssds: Vec<SsdTemperature>,
}

pub mod collection;
pub use collection::CollectionPolicy;
pub mod control;
pub use control::{AgentControl, AgentControlRequest, AgentControlResponse};
pub mod network;
pub use network::{NetworkActivity,NetworkGroup,NetworkGroupMetric};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MetricsSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_control: Option<AgentControl>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub network_activity: Option<NetworkActivity>,
    #[serde(default)]
    pub hardware_details: Option<HardwareDetails>,
    #[serde(default)]
    pub temperatures: Option<HostTemperatures>,
    #[serde(default)]
    pub collection_policy: Option<CollectionPolicy>,
    #[serde(default)]
    pub fast_collected_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub gpu_collected_at: Option<DateTime<Utc>>,
    pub collected_at: DateTime<Utc>,
    pub cpu_usage_percent: f32,
    pub load_average_one: f64,
    pub load_average_five: f64,
    pub load_average_fifteen: f64,
    pub memory_total_bytes: u64,
    pub memory_used_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub uptime_seconds: u64,
    pub disks: Vec<DiskMetric>,
    #[serde(default)]
    pub block_devices: Vec<BlockDevice>,
    #[serde(default)]
    pub disk_collected_at: Option<DateTime<Utc>>,
    pub networks: Vec<NetworkMetric>,
    pub gpus: Vec<GpuMetric>,
    #[serde(default)]
    pub platform_process: Option<PlatformProcessMetric>,
    #[serde(default)]
    pub platform_storage: Option<PlatformStorageMetric>,
    #[serde(default)]
    pub privilege: Option<AgentPrivilege>,
    #[serde(default)]
    pub users: Vec<UserStatus>,
    #[serde(default)]
    pub interfaces: Vec<NetworkInterface>,
    #[serde(default)]
    pub overlay_networks: Vec<OverlayNetworkStatus>,
    #[serde(default)]
    pub routes: Vec<RouteInfo>,
    #[serde(default)]
    pub proxy_services: Vec<ProxyService>,
    #[serde(default)]
    pub collection_issues: Vec<CollectionIssue>,
    #[serde(default)]
    pub inventory_collected_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub software_packages: Vec<SoftwarePackage>,
    #[serde(default)]
    pub running_services: Vec<ServiceStatus>,
    #[serde(default)]
    pub host_security: Option<HostSecurity>,
    #[serde(default)]
    pub permission_audit: Option<PermissionAudit>,
    #[serde(default)]
    pub ai_tools: Vec<AiToolStatus>,
    #[serde(default)]
    pub programming_tools: Vec<ProgrammingToolStatus>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnrollmentRequest {
    pub protocol_version: u16,
    pub requested_node_id: Uuid,
    pub agent_version: String,
    pub inventory: NodeInventory,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnrollmentResponse {
    pub node_id: Uuid,
    pub agent_token: String,
    pub report_interval_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentReport {
    pub protocol_version: u16,
    pub node_id: Uuid,
    pub agent_version: String,
    pub inventory: NodeInventory,
    pub metrics: MetricsSnapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentReportResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_control: Option<AgentControl>,
    pub accepted_at: DateTime<Utc>,
    #[serde(default)]
    pub collection_policy: Option<CollectionPolicy>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LiveMetrics {
    pub collected_at: DateTime<Utc>,
    pub cpu_usage_percent: f32,
    pub memory_total_bytes: u64,
    pub memory_used_bytes: u64,
    pub network_received_bytes_per_second: f64,
    pub network_transmitted_bytes_per_second: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentLiveReport {
    pub protocol_version: u16,
    pub node_id: Uuid,
    pub metrics: LiveMetrics,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LiveNode {
    pub node_id: Uuid,
    pub received_at: DateTime<Utc>,
    pub metrics: LiveMetrics,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LiveNodeListResponse {
    pub generated_at: DateTime<Utc>,
    pub nodes: Vec<LiveNode>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NodeSummary {
    #[serde(default)]
    pub monitoring_paused: bool,
    #[serde(default)]
    pub monitoring_revision: u64,
    #[serde(default)]
    pub monitoring_applied_revision: Option<u64>,
    pub id: Uuid,
    pub hostname: String,
    pub os_name: String,
    pub os_version: String,
    pub kernel_version: String,
    pub architecture: String,
    pub cpu_model: String,
    pub cpu_logical_cores: u32,
    pub cpu_physical_cores: Option<u32>,
    pub agent_version: String,
    pub last_seen: DateTime<Utc>,
    pub online: bool,
    pub metrics: Option<MetricsSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NodeListResponse {
    pub generated_at: DateTime<Utc>,
    pub nodes: Vec<NodeSummary>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
    pub version: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}
