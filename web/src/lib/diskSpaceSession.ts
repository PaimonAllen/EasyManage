import type {SpaceStorage} from "../api/diskSpace";
import type {SpaceBasis,SpaceSort} from "./diskSpace";
const KEY="easymanage.disk-space.session.v1";
export const SESSION_TTL=24*60*60*1000;
export interface MachineSession {
  storage:SpaceStorage;scans:Partial<Record<SpaceStorage,string>>;path:string;customPath:boolean;
  scope:"directory"|"disk"|"all_local";diskKey:string;seconds:number;maxEntries:number;
}
export interface SpaceViewSession {treemapCollapsed?:boolean;directory:number;basis:SpaceBasis;sort:SpaceSort;ascending:boolean;search:string;offset:number;expanded:number[];linked?:number[]}
interface Session {client:string;expires:number;node:string;machines:Record<string,MachineSession>;views:Record<string,SpaceViewSession>;links?:Record<string,Record<string,{id:string;path:string}>>}
export function readSpaceSession():Session|null {
  try{
    if(typeof window==="undefined")return null;
    const value=JSON.parse(window.localStorage.getItem(KEY)??"null");
    if(!value||typeof value.client!=="string"||!Number.isFinite(value.expires)||value.expires<=Date.now()
      ||typeof value.node!=="string"||!value.machines||typeof value.machines!=="object"||!value.views||typeof value.views!=="object"){
      window.localStorage.removeItem(KEY);return null;
    }
    return value as Session;
  }catch{return null;}
}
function update(edit:(session:Session)=>void):void {
  if(typeof window==="undefined")return;
  const session=readSpaceSession()??{client:crypto.randomUUID(),expires:0,node:"",machines:{},views:{}};
  edit(session);session.expires=Date.now()+SESSION_TTL;
  try{window.localStorage.setItem(KEY,JSON.stringify(session));}catch{/* Disabled/full browser storage must never stop a scan. */}
}
export function touchSpaceSession():void {update(()=>{});}
export function rememberSpaceNode(node:string):void {update(s=>{s.node=node;});}
export function readMachineSession(node:string):MachineSession|undefined {
  const value=readSpaceSession()?.machines[node];
  if(!value||!["snapshot","realtime"].includes(value.storage)||!["directory","disk","all_local"].includes(value.scope)
    ||typeof value.path!=="string"||value.path.length>2048||typeof value.customPath!=="boolean"
    ||typeof value.diskKey!=="string"||typeof value.scans!=="object"||!value.scans
    ||![0,30,60,120].includes(value.seconds)||![0,20000,100000].includes(value.maxEntries)
    ||Object.values(value.scans).some(id=>typeof id!=="string"||id.length>128))return;
  return value;
}
export function rememberMachineSession(node:string,value:MachineSession):void {update(s=>{s.machines[node]=value;});}
const viewKey=(node:string,scan:string)=>node+":"+scan;
export function readSpaceView(node:string,scan:string):SpaceViewSession|undefined {
  const v=readSpaceSession()?.views[viewKey(node,scan)];
  if(!v||!Number.isSafeInteger(v.directory)||v.directory<0||!Number.isSafeInteger(v.offset)||v.offset<0
    ||!["allocated","apparent"].includes(v.basis)||!["size","name","percent","files"].includes(v.sort)||typeof v.ascending!=="boolean"
    ||typeof v.search!=="string"||v.search.length>128||!Array.isArray(v.expanded)||v.expanded.length>8
    ||(v.treemapCollapsed!==undefined&&typeof v.treemapCollapsed!=="boolean")
    ||v.expanded.some(id=>!Number.isSafeInteger(id)||id<0)
    ||(v.linked!==undefined&&(!Array.isArray(v.linked)||v.linked.length>8||v.linked.some(id=>!Number.isSafeInteger(id)||id<0))))return;
  return v;
}
export function rememberSpaceView(node:string,scan:string,value:SpaceViewSession):void {update(s=>{s.views[viewKey(node,scan)]=value;});}

export function readLinkedScans(node:string,scan:string):Record<string,{id:string;path:string}>{
  const links=readSpaceSession()?.links?.[viewKey(node,scan)];
  if(!links||typeof links!=="object")return {};
  return Object.fromEntries(Object.entries(links).filter(([key,value])=>/^\d+$/.test(key)&&value&&typeof value.id==="string"&&value.id.length<=128&&typeof value.path==="string"&&value.path.length<=2048));
}
export function rememberLinkedScan(node:string,scan:string,entry:number,value:{id:string;path:string}):void{
  update(s=>{s.links??={};s.links[viewKey(node,scan)]??={};s.links[viewKey(node,scan)][String(entry)]=value;});
}
