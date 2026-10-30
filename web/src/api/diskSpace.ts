export interface SpaceEntry {
  id:number; parent:number|null; name:string; kind:"directory"|"file"|"symlink"|"special";
  allocated:number; apparent:number; files:number; directories:number; issue:string|null; incomplete:boolean; mtime:number; scan_path?:string|null;
}
export interface SpaceResult {
  root:string; uid:number; username:string; status:string; reason:string; elapsed_seconds:number;
  filesystem:string; visited:number; errors:number; skipped_mounts:number; symlinks:number; hardlinks:number; nodes:SpaceEntry[]; paged?:boolean;
  scope?:"directory"|"all_local";filesystems?:{id:number;path:string;filesystem:string;incomplete:boolean}[];
  detail_level?:"directories";saved_directories?:number;
}
export interface SpaceScan {
  id:string; mode:"basic"|"full"; root_path:string; status:string; started_at:string; finished_at:string|null;
  scope?:"directory"|"all_local";
  progress:{visited?:number;errors?:number;elapsed_seconds?:number}; message:string; result?:SpaceResult|null;
}
export interface SpaceStatus {scans:SpaceScan[];username:string;default_path:string;root_authorized:boolean;limits:{seconds:number|null;entries:number|null;depth:number;concurrent:number;retention_days:number}}
export interface SpacePage {directory:SpaceEntry;ancestors:SpaceEntry[];entries:SpaceEntry[];top:SpaceEntry[];total:number;offset:number;limit:number;direct?:{allocated:number;apparent:number;files:number}}
async function request<T>(path:string, body?:unknown, signal?:AbortSignal):Promise<T> {
  const response=await fetch(path,{method:body===undefined?"GET":"POST",cache:"no-store",signal:signal??AbortSignal.timeout(15_000),
    headers:body===undefined?undefined:{"Content-Type":"application/json"},body:body===undefined?undefined:JSON.stringify(body)});
  if(!response.ok){let message=`磁盘扫描接口异常（HTTP ${response.status}）`;try{message=(await response.json()).message??message;}catch{/* fixed HTTP fallback */}throw new Error(message);}
  return response.json() as Promise<T>;
}
const base=(node:string)=>`/api/v1/nodes/${encodeURIComponent(node)}/disk-space`;
export const fetchSpaceStatus=(node:string,signal?:AbortSignal)=>request<SpaceStatus>(base(node),undefined,signal);
export const fetchSpaceScan=(node:string,id:string,signal?:AbortSignal)=>request<SpaceScan>(`${base(node)}/${encodeURIComponent(id)}`,undefined,signal);
export const startSpaceScan=(node:string,path:string,mode:"basic"|"full",seconds:number,maxEntries=0,scope:"directory"|"all_local"="directory")=>request<{id:string}>(base(node),{path,mode,seconds,max_entries:maxEntries,scope});
export const fetchSpacePage=(node:string,id:string,parent:number,basis:string,sort:string,ascending:boolean,offset:number,search:string,signal?:AbortSignal)=>{
  const query=new URLSearchParams({parent:String(parent),basis,sort,ascending:String(ascending),offset:String(offset),search});
  return request<SpacePage>(`${base(node)}/${encodeURIComponent(id)}/entries?${query}`,undefined,signal);
};
export const cancelSpaceScan=(node:string,id:string)=>request<{message:string}>(`${base(node)}/${encodeURIComponent(id)}/cancel`,{});
