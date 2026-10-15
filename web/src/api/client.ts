import type { LiveNodeListResponse, NodeListResponse, NodeMetricHistory, PermissionCheck, PlatformProcessMetric, PlatformStorageMetric } from "../types/monitoring";
import type { CollectionPolicy } from "../types/monitoring";
import { serializeMachineRequest, type MachineAuthentication } from "../lib/machineAuthentication";

export interface CollectionSettingsStatus {
  policy: CollectionPolicy;
  items: { key: Exclude<keyof CollectionPolicy, "revision">; title: string; description: string; minimum: number; maximum: number; default_seconds: number }[];
  agents: { node_id: string; hostname: string; last_seen: string; applied_policy: CollectionPolicy | null }[];
}

export async function fetchCollectionSettings(signal?: AbortSignal): Promise<CollectionSettingsStatus> {
  return readResponse<CollectionSettingsStatus>(await fetch("/api/v1/collection-settings", { cache: "no-store", signal }));
}

export async function saveCollectionSettings(policy: CollectionPolicy): Promise<{ policy: CollectionPolicy; message: string }> {
  return readResponse(await fetch("/api/v1/collection-settings", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(policy), cache: "no-store", signal: AbortSignal.timeout(15_000) }));
}

export interface PermissionReview {
  node_id: string;
  check: PermissionCheck;
  note: string;
  marked_at: string;
}

export async function fetchPermissionReviews(signal?: AbortSignal): Promise<PermissionReview[]> {
  const response = await readResponse<{ reviews: PermissionReview[] }>(await fetch("/api/v1/permission-reviews", { cache: "no-store", signal }));
  return response.reviews;
}

export async function markPermissionReview(nodeId: string, check: PermissionCheck, accepted: boolean, note: string): Promise<void> {
  await readResponse(await fetch(`/api/v1/nodes/${encodeURIComponent(nodeId)}/permission-review`, {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ check, accepted, note }), cache: "no-store", signal: AbortSignal.timeout(15_000),
  }));
}

export interface PlatformConfig {
  documentation_url: string;
  operations_enabled: boolean;
}

export interface OperationTarget {
  node_id: string;
  hostname: string;
  host: string;
  port: number;
  default_user: string;
  elevation_available: boolean;
  update_available?: boolean;
  update_state?: "available" | "current" | "not_published" | "untrusted_target" | "unknown_installed";
  restart_available?: boolean;
}

export interface ManagedMachine {
  node_id: string; hostname: string; display_name: string | null; note: string;
  deleted_at: string | null; last_seen: string; host: string | null; port: number | null;
  username: string | null; manageable: boolean;
}
export interface MachineStatus {
  machines: ManagedMachine[]; candidate_available: boolean; ca_available: boolean; registry_writable: boolean;
}
export interface MachineDeployment {
  node_id: string; status: "running" | "success" | "failed"; progress: number;
  events: { at: string; message: string }[];
}
export async function fetchMachines(signal?: AbortSignal): Promise<MachineStatus> {
  return readResponse(await fetch("/api/v1/machines", { cache: "no-store", signal }));
}
export async function probeMachine(host: string, port: number): Promise<{fingerprint: string; message: string}> {
  return readResponse(await fetch("/api/v1/machines/probe", { method: "POST", headers: {"Content-Type":"application/json"},
    body: JSON.stringify({host, port}), cache:"no-store", signal:AbortSignal.timeout(30_000) }));
}
export async function addMachine(request: {host: string; port: number; display_name: string; server_url: string; fingerprint: string; username: string} & MachineAuthentication): Promise<{job_id: string; job_key: string; node_id: string}> {
  const body = serializeMachineRequest(request);
  return readResponse(await fetch("/api/v1/machines", { method:"POST", headers:{"Content-Type":"application/json"},
    body, cache:"no-store", signal:AbortSignal.timeout(20_000) }));
}
export async function fetchMachineJob(id: string, key: string, signal?: AbortSignal): Promise<MachineDeployment> {
  return readResponse(await fetch(`/api/v1/machines/jobs/${encodeURIComponent(id)}`, { headers:{Authorization:`Bearer ${key}`}, cache:"no-store", signal }));
}
export async function changeMachine(id: string, request: {action: "edit"|"delete"|"restore"; display_name: string; note: string; confirmation: string; username: string; password: string; auth_scope: "target"|"platform_admin"}): Promise<{message: string}> {
  return readResponse(await fetch(`/api/v1/machines/${encodeURIComponent(id)}/manage`, { method:"POST", headers:{"Content-Type":"application/json"},
    body:JSON.stringify(request), cache:"no-store", signal:AbortSignal.timeout(45_000) }));
}

export interface RecoveryStatus {
  database_healthy: boolean;
  platform_restart_available: boolean;
  message: string;
}

export async function fetchRecoveryStatus(signal?: AbortSignal): Promise<RecoveryStatus> {
  return readResponse(await fetch("/api/v1/operations/recovery", { cache: "no-store", signal }));
}

export async function forceRestartService(nodeId: string | null, credentials: { username: string; password: string }, signal?: AbortSignal): Promise<{ status: string; message: string }> {
  const path = nodeId === null ? "/api/v1/platform/force-restart" : `/api/v1/nodes/${encodeURIComponent(nodeId)}/agent/force-restart`;
  return readResponse(await fetch(path, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(credentials), cache: "no-store", signal }));
}

export interface PlatformUpdateStatus {
  supported: boolean;
  update_available: boolean;
  candidate_sha256?: string | null;
  published_at?: string | null;
  message: string;
  last_result?: {status: "scheduled" | "installing" | "success" | "failed"; sha256: string; message?: string};
}

export async function fetchPlatformUpdateStatus(signal?: AbortSignal): Promise<PlatformUpdateStatus> {
  return readResponse(await fetch("/api/v1/platform/update", {cache: "no-store", signal}));
}

export async function updatePlatform(credentials: {username: string; password: string}, candidateSha256: string, signal?: AbortSignal): Promise<{message: string}> {
  return readResponse(await fetch("/api/v1/platform/update", {method: "POST", headers: {"Content-Type": "application/json"},
    body: JSON.stringify({...credentials, candidate_sha256: candidateSha256}), cache: "no-store", signal}));
}

export interface PlatformHealthStatus {
  generated_at: string;
  platform: { api_healthy: boolean; database_healthy: boolean; database_latency_ms: number; version: string };
  platform_process: PlatformProcessMetric | null;
  platform_process_collected_at?: string | null;
  platform_hostname: string;
  nodes: { node_id: string; hostname: string; agent_version: string; last_seen: string; agent_to_platform_healthy: boolean; agent_process: PlatformProcessMetric | null; agent_process_collected_at: string | null }[];
  platform_to_nodes: { node_id: string; hostname: string; host: string; port: number; ssh_tcp_reachable: boolean }[];
  target_error: string | null;
  check_meaning: string;
}

export interface ProcessMetricSummary {
  average: number | null;
  maximum: number | null;
  samples: number;
}

export interface ProcessWindowSummary {
  sample_count: number;
  first_sample_at: string | null;
  last_sample_at: string | null;
  cpu_percent_of_host: ProcessMetricSummary;
  resident_memory_bytes: ProcessMetricSummary;
  disk_read_bytes_per_second: ProcessMetricSummary;
  disk_written_bytes_per_second: ProcessMetricSummary;
}

export interface StorageWindowSummary {
  total_bytes: ProcessMetricSummary;
  last_sample_at: string | null;
  latest: PlatformStorageMetric | null;
}

export interface PlatformPerformanceStatus {
  generated_at: string;
  window_start: string;
  window_end: string;
  period_seconds: number;
  server: { hostname: string; stats: ProcessWindowSummary; storage?: StorageWindowSummary | null };
  agents: { node_id: string; hostname: string; online: boolean; last_seen: string; stats: ProcessWindowSummary; storage?: StorageWindowSummary | null }[];
}

export interface GiteaStatus {
  generated_at: string;
  healthy: boolean;
  message: string;
  version: string | null;
  latency_ms: number;
  checks: Record<string, { status: string; time?: string }[]> | null;
  source: string;
}

export interface GiteaAccount {
  id: number;
  login: string;
  full_name: string | null;
  created: string | null;
  last_login: string | null;
  is_admin: boolean | null;
  account_status: "normal" | "inactive" | "prohibited" | "restricted" | "unknown";
}

export interface GiteaAccounts {
  generated_at: string;
  scope: "complete" | "partial" | "public" | "unavailable";
  users: GiteaAccount[];
  total_count: number | null;
  truncated: boolean;
  message: string;
}

export interface ManagedUser {
  username: string;
  uid: number;
  gid: number;
  home: string;
  shell: string;
  display_name: string;
  account_status?: "normal" | "restricted" | "invalid" | null;
}

export interface UserOperation {
  username: string;
  password: string;
  action: "list" | "create" | "update" | "delete";
  subject?: string;
  display_name?: string;
  shell?: string;
  groups?: string[];
  confirmation?: string;
}

async function readResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    let message = `平台接口异常（HTTP ${response.status}）`;
    try {
      const error = (await response.json()) as {
        message?: string;
        error?: string;
      };
      message = error.message ?? error.error ?? message;
    } catch {
      /* Keep the HTTP status when the response is not JSON. */
    }
    throw new Error(message);
  }
  return response.json() as Promise<T>;
}

export async function fetchNodes(
  signal?: AbortSignal,
): Promise<NodeListResponse> {
  const response = await fetch("/api/v1/nodes", {
    cache: "no-store",
    signal,
  });

  return readResponse<NodeListResponse>(response);
}

export async function fetchLiveNodes(signal?: AbortSignal): Promise<LiveNodeListResponse> {
  return readResponse<LiveNodeListResponse>(
    await fetch("/api/v1/nodes/live", { cache: "no-store", signal }),
  );
}

export async function fetchNodeHistory(nodeId: string, signal?: AbortSignal): Promise<NodeMetricHistory> {
  return readResponse<NodeMetricHistory>(
    await fetch(`/api/v1/nodes/${encodeURIComponent(nodeId)}/history`, { cache: "no-store", signal }),
  );
}

export async function fetchPlatform(
  signal?: AbortSignal,
): Promise<PlatformConfig> {
  return readResponse<PlatformConfig>(
    await fetch("/api/v1/platform", { cache: "no-store", signal }),
  );
}

export async function fetchTargets(
  signal?: AbortSignal,
): Promise<OperationTarget[]> {
  const result = await readResponse<{ targets: OperationTarget[] }>(
    await fetch("/api/v1/operations/targets", { cache: "no-store", signal }),
  );
  return result.targets;
}

export async function fetchPlatformHealth(signal?: AbortSignal): Promise<PlatformHealthStatus> {
  return readResponse<PlatformHealthStatus>(await fetch("/api/v1/platform-health", { cache: "no-store", signal }));
}

export async function fetchPlatformPerformance(signal?: AbortSignal): Promise<PlatformPerformanceStatus | null> {
  const response = await fetch("/api/v1/platform-performance", { cache: "no-store", signal });
  // A static frontend may be published before the new controller and migration.
  if (response.status === 404) return null;
  return readResponse<PlatformPerformanceStatus>(response);
}

export async function fetchGitea(signal?: AbortSignal): Promise<GiteaStatus> {
  return readResponse<GiteaStatus>(await fetch("/api/v1/gitea", { cache: "no-store", signal }));
}

export async function fetchGiteaAccounts(signal?: AbortSignal): Promise<GiteaAccounts | null> {
  const response = await fetch("/api/v1/gitea/users", { cache: "no-store", signal });
  // A frontend-only publish may precede the control-plane update.
  if (response.status === 404) return null;
  return readResponse<GiteaAccounts>(response);
}

export async function manageUser(nodeId: string, operation: UserOperation, signal?: AbortSignal): Promise<{ accounts?: ManagedUser[]; status?: string; message?: string }> {
  return readResponse(await fetch(`/api/v1/nodes/${encodeURIComponent(nodeId)}/users/manage`, {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify(operation), cache: "no-store", signal,
  }));
}

export async function updateAgent(nodeId: string, credentials?: { username: string; password: string }, signal?: AbortSignal): Promise<{ status: string; message: string }> {
  return readResponse(await fetch(`/api/v1/nodes/${encodeURIComponent(nodeId)}/agent/update`, {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify(credentials ?? {}), cache: "no-store", signal,
  }));
}

export async function restartAgent(nodeId: string, credentials?: { username: string; password: string }, signal?: AbortSignal): Promise<{ status: string; message: string }> {
  return readResponse(await fetch(`/api/v1/nodes/${encodeURIComponent(nodeId)}/agent/restart`, {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify(credentials ?? {}), cache: "no-store", signal,
  }));
}

export async function elevateNode(
  nodeId: string,
  username: string,
  password: string,
  signal?: AbortSignal,
): Promise<{ message?: string }> {
  return readResponse(
    await fetch(`/api/v1/nodes/${encodeURIComponent(nodeId)}/elevate`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username, password }),
      cache: "no-store",
      signal,
    }),
  );
}
