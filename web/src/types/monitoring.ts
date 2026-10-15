export interface DiskMetric {
  name: string;
  mount_point: string;
  file_system: string;
  total_bytes: number;
  available_bytes: number;
  read_bytes_per_second?: number | null;
  written_bytes_per_second?: number | null;
}

export interface BlockDevice {
  name: string;
  path: string | null;
  kind: string;
  size_bytes: number;
  filesystem: string | null;
  label: string | null;
  mount_points: string[];
  model: string | null;
  transport: string | null;
  rotational: boolean | null;
  removable: boolean | null;
  read_only: boolean | null;
  partition_table: string | null;
  partition_label: string | null;
  start_bytes: number | null;
  children: BlockDevice[];
}

export interface AgentPrivilege {
  mode: string;
  username: string;
  uid: number;
}
export interface UserStatus {
  username: string;
  uid: number;
  gid: number;
  home: string;
  shell: string;
  groups: string[];
  sessions: number | null;
  session_sources?: string[];
  session_evidence?: {
    terminal_records: number | null;
    logind_sessions: number | null;
    ssh_processes: number | null;
  } | null;
  process_count: number | null;
  locked: boolean | null;
  account_status?: "normal" | "restricted" | "invalid" | null;
}
export interface NetworkInterface {
  name: string;
  kind: string;
  state: string;
  addresses: string[];
  mac_address: string | null;
  mtu: number | null;
  vlan_id: number | null;
  master: string | null;
}
export interface OverlayMembership {
  network_id: string;
  name: string | null;
  state: string;
  interface: string | null;
}
export interface OverlayNetworkStatus {
  provider: "tailscale" | "zerotier";
  state: "healthy" | "not_joined" | "degraded" | "offline" | "unknown" | "not_installed";
  detail: string;
  networks: OverlayMembership[];
}
export interface PlatformProcessMetric {
  pid: number;
  cpu_percent_of_host: number | null;
  resident_memory_bytes: number | null;
  disk_read_bytes_per_second: number | null;
  disk_written_bytes_per_second: number | null;
}

export interface PlatformStorageMetric {
  collected_at: string;
  binary_bytes: number | null;
  state_bytes: number | null;
  web_bytes: number | null;
  database_bytes: number | null;
  total_bytes: number | null;
}
export interface RouteInfo {
  destination: string;
  gateway: string | null;
  interface: string | null;
  metric: number | null;
}
export interface ProxyService {
  name: string;
  pid: number;
  username: string;
  uid: number;
  listen_addresses: string[];
}
export interface CollectionIssue {
  component: string;
  message: string;
}
export interface SoftwarePackage {
  name: string;
  version: string;
}
export interface ServiceStatus {
  name: string;
  state: string;
}
export interface HostSecurity {
  apparmor_enabled: boolean | null;
  aslr_mode: number | null;
  ptrace_scope: number | null;
  kernel_lockdown: string | null;
}
export interface PermissionCheck {
  category: "home" | "shared" | "project" | "sudo" | "root";
  title: string;
  path: string;
  status: "ok" | "warning" | "critical" | "unknown" | "missing";
  observed: string;
  expected: string;
  detail: string;
}
export interface PermissionAudit {
  collected_at: string;
  checks: PermissionCheck[];
  truncated: boolean;
}
export interface AiToolProcess {
  pid: number;
  uid: number;
  username: string;
}
export interface AiToolStatus {
  name: string;
  category: "assistant" | "runtime";
  executable_paths: string[];
  processes: AiToolProcess[];
  service_units: string[];
}
export interface ProgrammingToolStatus {
  name: string;
  category: string;
  executable_paths: string[];
  package_version: string | null;
}

export interface NetworkMetric {
  interface: string;
  received_bytes_per_second: number;
  transmitted_bytes_per_second: number;
  total_received_bytes: number;
  total_transmitted_bytes: number;
}

export interface GpuMetric {
  index: number;
  name: string;
  utilization_percent: number;
  memory_total_bytes: number;
  memory_used_bytes: number;
  temperature_celsius: number | null;
  power_watts: number | null;
}

export interface CollectionPolicy {
  revision: number;
  live_seconds: number;
  report_seconds: number;
  gpu_seconds: number;
  disk_seconds: number;
  extended_seconds: number;
  permission_seconds: number;
  node_seconds: number;
  server_process_seconds: number;
  platform_health_seconds: number;
  gitea_seconds: number;
}

export interface HardwareDetails {
  collected_at: string;
  processors: string[];
  motherboard: string[];
  memory: string[];
  graphics: string[];
  displays: string[];
  disks: string[];
  audio: string[];
  network: string[];
  notes: string[];
}

export interface MetricsSnapshot {
  hardware_details?: HardwareDetails | null;
  temperatures?: HostTemperatures | null;
  collection_policy?: CollectionPolicy | null;
  fast_collected_at?: string | null;
  gpu_collected_at?: string | null;
  collected_at: string;
  cpu_usage_percent: number;
  load_average_one: number;
  load_average_five: number;
  load_average_fifteen: number;
  memory_total_bytes: number;
  memory_used_bytes: number;
  swap_total_bytes: number;
  swap_used_bytes: number;
  uptime_seconds: number;
  disks: DiskMetric[];
  block_devices?: BlockDevice[];
  disk_collected_at?: string | null;
  networks: NetworkMetric[];
  gpus: GpuMetric[];
  platform_process?: PlatformProcessMetric | null;
  privilege?: AgentPrivilege | null;
  users?: UserStatus[];
  interfaces?: NetworkInterface[];
  overlay_networks?: OverlayNetworkStatus[];
  routes?: RouteInfo[];
  proxy_services?: ProxyService[];
  collection_issues?: CollectionIssue[];
  inventory_collected_at?: string | null;
  software_packages?: SoftwarePackage[];
  running_services?: ServiceStatus[];
  host_security?: HostSecurity | null;
  permission_audit?: PermissionAudit | null;
  ai_tools?: AiToolStatus[];
  programming_tools?: ProgrammingToolStatus[];
}

export interface NodeSummary {
  id: string;
  hostname: string;
  os_name: string;
  os_version: string;
  kernel_version: string;
  architecture: string;
  cpu_model: string;
  cpu_logical_cores: number;
  cpu_physical_cores: number | null;
  agent_version: string;
  last_seen: string;
  online: boolean;
  metrics: MetricsSnapshot | null;
}

export interface NodeListResponse {
  generated_at: string;
  nodes: NodeSummary[];
}

export interface LiveMetrics {
  collected_at: string;
  cpu_usage_percent: number;
  memory_total_bytes: number;
  memory_used_bytes: number;
  network_received_bytes_per_second: number;
  network_transmitted_bytes_per_second: number;
}

export interface LiveNode {
  node_id: string;
  received_at: string;
  metrics: LiveMetrics;
}

export interface LiveNodeListResponse {
  generated_at: string;
  nodes: LiveNode[];
}

export interface MetricHistoryPoint {
  temperatures?: HostTemperatures | null;
  fast_collected_at?: string | null;
  gpu_collected_at?: string | null;
  collected_at: string;
  cpu_usage_percent: number;
  memory_used_bytes: number;
  disk_collected_at?: string | null;
  disk_read_bytes_per_second?: number | null;
  disk_written_bytes_per_second?: number | null;
  network_received_bytes_per_second: number;
  network_transmitted_bytes_per_second: number;
  gpus: (Pick<GpuMetric, "index" | "utilization_percent" | "memory_used_bytes" | "memory_total_bytes"> & { temperature_celsius?: number | null })[];
}

export interface HostTemperatures {
  collected_at: string;
  cpu_celsius: number | null;
  ssds: { device: string; temperature_celsius: number }[];
}

export interface NodeMetricHistory {
  node_id: string;
  generated_at: string;
  window_seconds: number;
  points: MetricHistoryPoint[];
}
