import type {BlockDevice,DiskMetric} from "../types/monitoring";

export function isBootMount(path:string):boolean {
  const value=path.replace(/\/+$/,"");
  return value==="/boot"||value.startsWith("/boot/")||value==="/efi";
}
export function isBootPartition(device:BlockDevice):boolean {
  if(device.kind!=="part"||device.mount_points.includes("/"))return false;
  return device.mount_points.some(isBootMount)||/^(efi(?: system(?: partition)?)?|esp|bios boot(?: partition)?|boot|启动分区|efi 系统分区)$/i.test((device.partition_label??device.label??"").trim());
}
export const isNetworkStorage=(disk:DiskMetric)=>disk.mount?.network??/^(nfs4?|cifs|smb3|(?:fuse\.)?sshfs|9p|ceph|(?:fuse\.)?glusterfs)$/.test(disk.file_system);
export const capacityKnown=(disk:DiskMetric)=>disk.mount?.capacity_known!==false&&Number.isFinite(disk.total_bytes)&&disk.total_bytes>0&&Number.isFinite(disk.available_bytes)&&disk.available_bytes>=0&&disk.available_bytes<=disk.total_bytes;
export const storageMountKey=(disk:DiskMetric)=>`${disk.name}\0${disk.mount_point}`;
export interface SharedStorage {members:DiskMetric[];primary:DiskMetric;precise:boolean}
export function sharedStorage(disks:DiskMetric[]):Map<string,SharedStorage>{
  const groups=new Map<string,DiskMetric[]>();
  for(const disk of disks){
    const identity=disk.mount?.device_id?`id:${disk.mount.device_id}:${disk.file_system}`:
      disk.name.startsWith("/dev/")||isNetworkStorage(disk)?`source:${disk.name}:${disk.file_system}`:null;
    if(!identity)continue;
    const members=groups.get(identity)??[];members.push(disk);groups.set(identity,members);
  }
  const result=new Map<string,SharedStorage>();
  for(const members of groups.values()){
    if(members.length<2)continue;
    const ordered=[...members].sort((a,b)=>Number(b.mount?.root==="/")-Number(a.mount?.root==="/")||a.mount_point.length-b.mount_point.length||a.mount_point.localeCompare(b.mount_point));
    const group={members:ordered,primary:ordered[0],precise:members.every(d=>!!d.mount)};
    for(const disk of members)result.set(storageMountKey(disk),group);
  }
  return result;
}

// Only mounted local filesystems supported by the directory scanner. Overlay
// mounts and network/virtual filesystems are not physical disk choices.
const scanFilesystems=new Set(['ext2','ext3','ext4','xfs','btrfs','zfs','f2fs','jfs','reiserfs','ntfs','ntfs3','fuseblk','exfat','vfat','hfsplus','bcachefs']);
export interface ScanDiskTarget {key:string;disk:DiskMetric;aliases:number;subtree:boolean}
export function scanDiskTargets(disks:DiskMetric[],devices:BlockDevice[]=[],showBoot=true):ScanDiskTarget[]{
  const bootDevices=new Set<string>();
  function collectBoot(items:BlockDevice[]){for(const d of items){if(isBootPartition(d))bootDevices.add(d.path??`/dev/${d.name}`);collectBoot(d.children);}}
  collectBoot(devices);
  const candidates=disks.filter(d=>scanFilesystems.has(d.file_system)&&!isNetworkStorage(d)&&d.mount_point.startsWith('/')&&
    (showBoot||d.mount_point==='/'||(!isBootMount(d.mount_point)&&!bootDevices.has(d.name))));
  const groups=new Map<string,DiskMetric[]>();
  for(const disk of candidates){
    const identity=JSON.stringify([disk.mount?.device_id??disk.name,disk.file_system]);
    const group=groups.get(identity)??[];group.push(disk);groups.set(identity,group);
  }
  const targets:ScanDiskTarget[]=[];
  const root=(d:DiskMetric)=>d.mount?.root??'/';
  const contains=(parent:string,child:string)=>parent===child||child.startsWith(parent.replace(/\/$/,'')+'/');
  for(const members of groups.values()){
    // Prefer the broadest mount root; preserve disjoint Btrfs subvolumes or
    // subtree-only mounts instead of silently hiding accessible data.
    const selected:ScanDiskTarget[]=[];
    for(const disk of [...members].sort((a,b)=>root(a).length-root(b).length||a.mount_point.length-b.mount_point.length||a.mount_point.localeCompare(b.mount_point))){
      const existing=selected.find(t=>contains(root(t.disk),root(disk)));
      if(existing){existing.aliases++;continue;}
      selected.push({key:JSON.stringify([disk.name,disk.mount_point,disk.mount?.device_id??null,root(disk),disk.file_system]),disk,aliases:0,subtree:root(disk)!=='/'});
    }
    targets.push(...selected);
  }
  return targets.sort((a,b)=>Number(b.disk.mount_point==='/')-Number(a.disk.mount_point==='/')||a.disk.mount_point.localeCompare(b.disk.mount_point));
}
