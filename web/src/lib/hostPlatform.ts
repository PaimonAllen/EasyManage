import type {AgentPrivilege,NodeSummary} from "../types/monitoring";
export function isWindows(node:NodeSummary):boolean {return node.metrics?.privilege?.native?.platform==="windows" || /windows/i.test(node.os_name);}
export function isFullPrivilege(privilege:AgentPrivilege|null|undefined):boolean {
 return privilege?.mode==="full" && (privilege.native?.platform==="windows" ? privilege.native.administrator===true : privilege.uid===0);
}
export function privilegeIdentity(privilege:AgentPrivilege):string {return privilege.native?.platform==="windows" ? `SID ${privilege.native.principal}` : `UID ${privilege.uid}`;}

export function canDeployPlatform(platform:"linux"|"windows",available:boolean,candidates?:{linux:boolean;windows:boolean}):boolean {return available&&(candidates?.[platform]??true);}
