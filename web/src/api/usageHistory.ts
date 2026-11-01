export interface UsageStat {average:number|null;maximum:number|null;minimum:number|null;samples:number}
export interface UsageThrottleStat {samples:number;detected_samples:number}
export interface UsageGpuStats {core:UsageStat;memory:UsageStat;memory_percent:UsageStat;temperature:UsageStat;thermal_throttling?:UsageThrottleStat}
export interface UsageStats {
  cpu:UsageStat;memory:UsageStat;network_rx:UsageStat;network_tx:UsageStat;disk_read:UsageStat;disk_write:UsageStat;
  cpu_temperature:UsageStat;ssd_temperature:UsageStat;gpu_count:UsageStat;gpu:UsageGpuStats;gpus:Record<string,UsageGpuStats>;cpu_thermal_throttling?:UsageThrottleStat;
}
export interface UsageDay {date:string;start:string;end:string;summary:UsageStats}
export interface UsagePoint {at:string;stats:UsageStats}
export interface UsageHistory {
  node_id:string;hostname:string;generated_at:string;start:string;end:string;utc_offset_minutes:number;
  bucket_seconds:number;cache_seconds:number;source_rows:number;summary:UsageStats;days:UsageDay[];
  gpus:{index:number;name:string;last_seen?:string}[];points:UsagePoint[];
}
export async function fetchUsageHistory(node:string,offset:number,signal:AbortSignal):Promise<UsageHistory>{
  const query=new URLSearchParams({utc_offset_minutes:String(offset)});
  const response=await fetch(`/api/v1/nodes/${encodeURIComponent(node)}/usage-history?${query}`,{cache:"no-store",signal:AbortSignal.any([signal,AbortSignal.timeout(15_000)])});
  if(!response.ok){let message=`历史接口异常（HTTP ${response.status}）`;try{message=(await response.json()).message??message;}catch{/* bounded HTTP fallback */}throw new Error(message);}
  return response.json() as Promise<UsageHistory>;
}
