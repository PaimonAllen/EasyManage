export interface ProxyMonitoringSettings {enabled:boolean;local_enabled:boolean;revision:number;global_epoch:number;local_epoch:number}
export interface ProxyProfile {id:string;name:string;source:string;original_filename?:string|null;duplicate_count?:number;revision:number;enabled:boolean;warnings:string[];node_count:number;last_check:string|null}
export interface ReputationProvider {id:string;name:string;url:string;api:boolean;direction:string}
export interface ProxyProbeSite {name:string;url:string;expected:string}
export interface ProxySiteFeedback {site:number;primary_status?:number|null;fallback_status?:number|null;retry_at?:string|null;fallback_retry_at?:string|null;rate_limited:boolean;outcome:string}
export interface ProxyLabStatus {monitoring?:ProxyMonitoringSettings;profiles:ProxyProfile[];providers:ReputationProvider[];check_seconds:number;window_hours:number;core_available:boolean;checking:boolean;checking_profile_id?:string|null;queue_mode?:string;score_busy:boolean;test_url:string;test_sites?:ProxyProbeSite[];legacy_test_sites?:ProxyProbeSite[];restricted_test_sites?:ProxyProbeSite[];restricted_fallback_sites?:ProxyProbeSite[];fallback_sites?:ProxyProbeSite[];checking_profile_ids?:string[];round?:{started_at:string|null;finished_at:string|null;duration_ms:number|null;expected_nodes:number;completed_nodes:number;saved_nodes?:number;timed_out_nodes?:number;next_run_at?:string|null;profile_ids:string[]};availability_policy?:number;pass_percent_exclusive?:number;pass_percent_inclusive?:number;retention_days?:number;probe_concurrency?:number|null;site_timeout_ms?:number}
export interface ReputationScore {provider:string;revision:number;ip:string|null;region:string|null;raw_score:number|null;purity:number|null;direction:string;state:string;detail:string;method:string;checked_at:string;observed_at:string|null}
export interface ProxyAvailabilitySeries {history:string;legacy_samples?:number;latest:{state:string;latency_ms:number|null;http_status?:number|null;http_fallback?:{head_status:number;get_status:number|null}|null;detail:string;policy?:number;sites?:number[];site_feedback?:ProxySiteFeedback[];link?:{state:string;latency_ms:number|null;detail:string;kind:string}}|null;last_check:string|null;available_samples:number;observed_samples:number;expected_samples:number;availability_percent:number|null;coverage_percent:number}
export interface ProxyLabNode extends ProxyAvailabilitySeries {id:string;name:string;protocol:string;region:string;unsupported:string|null;revision:number;node_link?:ProxyAvailabilitySeries;scores:ReputationScore[]}
export interface ProxyLabDetail {profile_id:string;window_start:string;generated_at:string;slot_seconds:number;nodes:ProxyLabNode[]}
export interface ReputationJob {id:string;status:string;total:number;completed:number;message:string;started_at:string}
export interface LocalProxySource {id:string;name:string;bytes:number}
const BASE="/api/v1/proxy-lab";
async function request<T>(path:string,signal?:AbortSignal,method="GET",body?:unknown):Promise<T>{
  const response=await fetch(BASE+path,{method,cache:"no-store",signal:signal??AbortSignal.timeout(15_000),
    ...(body===undefined?{}:{headers:{"Content-Type":"application/json"},body:JSON.stringify(body)})});
  const data=await response.json().catch(()=>({message:"平台代理分析接口返回无效响应"}));
  if(!response.ok)throw new Error(data.message||`请求失败（HTTP ${response.status}）`);
  return data as T;
}
export const fetchProxyLab=(signal?:AbortSignal)=>request<ProxyLabStatus>("",signal);
export const fetchProxySources=(signal?:AbortSignal)=>request<{sources:LocalProxySource[]}>("/sources",signal);
export const fetchProxyDetail=(id:string,signal?:AbortSignal)=>request<ProxyLabDetail>(`/profiles/${encodeURIComponent(id)}`,signal);
export async function fetchProxyDetails(ids:string[],signal:AbortSignal){
  const details:ProxyLabDetail[]=[];const failed:string[]=[];
  // Keep browser reads bounded; opening the overview never starts a probe.
  for(let offset=0;offset<ids.length;offset+=2){
    signal.throwIfAborted();const batch=ids.slice(offset,offset+2);
    const results=await Promise.allSettled(batch.map(id=>fetchProxyDetail(id,AbortSignal.any([signal,AbortSignal.timeout(12_000)]))));
    results.forEach((result,index)=>{if(result.status==="fulfilled")details.push(result.value);else failed.push(batch[index]);});
  }
  return {details,failed};
}
export const importProxyProfile=(input:{name:string;filename?:string;yaml?:string;source_id?:string;replace_id?:string;expected_revision?:number;enable:boolean},signal?:AbortSignal)=>request<{id:string;message:string}>("/profiles",signal,"POST",input);
export const renameProxyProfile=(profile:ProxyProfile,name:string,signal?:AbortSignal)=>request<{message:string}>(`/profiles/${profile.id}/name`,signal,"PUT",{name,expected_name:profile.name,expected_revision:profile.revision});
export const deleteProxyProfile=(profile:ProxyProfile,confirmation:string,signal?:AbortSignal)=>request<{id:string;message:string}>(`/profiles/${profile.id}`,signal,"DELETE",{expected_name:profile.name,expected_revision:profile.revision,confirmation});
export const enableProxyProfile=(profile:ProxyProfile,enabled:boolean,signal?:AbortSignal)=>request<{message:string}>(`/profiles/${profile.id}`,signal,"PUT",{enabled,expected_revision:profile.revision,expected_enabled:profile.enabled});
export const scoreProxyNodes=(profile:string,nodeIds:string[],signal?:AbortSignal)=>request<{job_id:string}>(`/profiles/${profile}/score`,signal,"POST",{node_ids:nodeIds});
export const fetchReputationJob=(id:string,signal?:AbortSignal)=>request<ReputationJob>(`/jobs/${id}`,signal);
export const saveManualReputation=(node:ProxyLabNode,provider:string,input:{ip:string;raw_score:number;direction:string;observed_at:string},signal?:AbortSignal)=>request<{message:string}>(`/nodes/${node.id}/scores/${provider}`,signal,"PUT",{...input,expected_revision:node.revision});

export interface ProxyHistoryNode {id:string;name:string;protocol:string;active:boolean;url?:string}
export interface ProxyHistoryData {source?:string;profile:{id:string;name:string;enabled:boolean};nodes:ProxyHistoryNode[];selected_node:ProxyHistoryNode|null;generated_at:string;window_start:string;days:number;slot_seconds:number;retention_days:number;https:ProxyAvailabilitySeries|null;node_link:ProxyAvailabilitySeries|null}
export interface ProxyHistoryEntry {slot:string;checked_at:string;revision:number;observation:NonNullable<ProxyAvailabilitySeries["latest"]>}
export interface ProxyHistoryLogs {entries:ProxyHistoryEntry[];has_more:boolean;next_before:string|null;generated_at:string;days:number;retention_days:number}
export const fetchProxyHistory=(profile:string,node:string,days:number,signal:AbortSignal)=>request<ProxyHistoryData>(`/profiles/${encodeURIComponent(profile)}/history?${new URLSearchParams({days:String(days),...(node?{node}:{})})}`,signal);
export const fetchProxyHistoryLogs=(profile:string,node:string,days:number,before:string|null,signal:AbortSignal)=>request<ProxyHistoryLogs>(`/profiles/${encodeURIComponent(profile)}/logs?${new URLSearchParams({node,days:String(days),limit:"50",...(before?{before}:{})})}`,AbortSignal.any([signal,AbortSignal.timeout(15_000)]));

export const LOCAL_NETWORK_PROFILE="local-network";
export interface LocalNetworkSite extends ProxyAvailabilitySeries {id:string;name:string;url:string}
export interface LocalNetworkData {source:"local_network";enabled:boolean;monitoring:ProxyMonitoringSettings;configured_enabled:boolean;global_enabled:boolean;operator_enabled:boolean;window_start:string;generated_at:string;check_seconds:number;retention_days:number;nodes:LocalNetworkSite[];round:{started_at:string|null;finished_at:string|null;next_run_at:string|null;duration_ms:number|null;expected_sites:number;completed_sites:number;saved_sites:number;error:string|null}}
export const fetchLocalNetwork=(signal:AbortSignal)=>request<LocalNetworkData>("/local",signal);
export const fetchLocalNetworkHistory=(node:string,days:number,signal:AbortSignal)=>request<ProxyHistoryData>(`/local/history?${new URLSearchParams({days:String(days),...(node?{node}:{})})}`,signal);
export const fetchLocalNetworkLogs=(node:string,days:number,before:string|null,signal:AbortSignal)=>request<ProxyHistoryLogs>(`/local/logs?${new URLSearchParams({node,days:String(days),limit:"50",...(before?{before}:{})})}`,AbortSignal.any([signal,AbortSignal.timeout(15_000)]));

export const setProxyMonitoring=(settings:ProxyMonitoringSettings,enabled:boolean)=>request<{monitoring:ProxyMonitoringSettings;message:string}>("/monitoring",undefined,"PUT",{enabled,expected_revision:settings.revision,confirmed:true});
export const setLocalNetworkMonitoring=(settings:ProxyMonitoringSettings,enabled:boolean)=>request<{monitoring:ProxyMonitoringSettings;message:string}>("/local/monitoring",undefined,"PUT",{enabled,expected_revision:settings.revision});
