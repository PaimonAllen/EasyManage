import type {BlockDevice} from "../types/monitoring.ts";

// Accept the original Windows spelling until every Agent has refreshed inventory.
export const isPartition=(device:Pick<BlockDevice,"kind">):boolean=>device.kind==="part"||device.kind==="partition";
const windowsPath=(path:string)=>/^(?:[a-z]:[\\/]|\\\\[?.]\\)/i.test(path);
export function isWindowsDevice(device:Pick<BlockDevice,"name"|"path"|"kind"|"mount_points">):boolean {
  return device.kind==="partition"||/^Disk \d+(?: partition \d+)?$/.test(device.name)||windowsPath(device.path??"")||device.mount_points.some(windowsPath);
}
export function deviceMountPoints(device:BlockDevice):string[]{
  // Earlier Windows inventories omitted drive-letter mounts but retained path.
  if(device.mount_points.length)return device.mount_points;
  return isPartition(device)&&/^[a-z]:[\\/]/i.test(device.path??"")?[device.path!]:[];
}
export function blockDeviceTitle(device:BlockDevice):string {
  if(!isWindowsDevice(device))return device.path??device.name;
  return device.name.replace(/^Disk (\d+) partition (\d+)$/,"磁盘 $1 · 分区 $2").replace(/^Disk (\d+)$/,"磁盘 $1");
}
export function solidState(device:Pick<BlockDevice,"kind"|"model"|"rotational">):boolean {
  // Prefer native media metadata. Older inventories may explicitly name an SSD;
  // do not infer SSD merely from NVMe, capacity or an unknown model family.
  return device.kind==="disk"&&(device.rotational===false||(device.rotational==null&&/\bSSD\b/i.test(device.model??"")));
}
export const windowsMountKey=(path:string):string=>path.replace(/\\/g,"/").replace(/\/+$/,"").toLowerCase();
