import assert from "node:assert/strict";
import {test} from "node:test";
import {isPartition,deviceMountPoints,blockDeviceTitle,solidState,windowsMountKey} from "../src/lib/storageTopology.ts";
import {isBootPartition} from "../src/lib/storageMounts.ts";
import {diskRecording} from "../src/lib/diskRecording.ts";
import type {BlockDevice} from "../src/types/monitoring.ts";
const part=(overrides:Partial<BlockDevice>={}):BlockDevice=>({name:"Disk 0 partition 3",path:"C:\\",kind:"partition",size_bytes:100,filesystem:"NTFS",label:null,mount_points:[],model:null,transport:null,rotational:null,removable:null,read_only:false,partition_table:null,partition_label:"Basic",start_bytes:0,children:[],...overrides});
test("canonical and legacy Windows partitions retain mounts and safe boot classification",()=>{
 assert.ok(isPartition(part()));assert.ok(isPartition(part({kind:"part"})));assert.ok(!isPartition(part({kind:"disk"})));
 assert.deepEqual(deviceMountPoints(part()),["C:\\"]);
 assert.deepEqual(deviceMountPoints(part({path:"\\\\?\\Volume{fixture}\\"})),[]);
 assert.deepEqual(deviceMountPoints(part({mount_points:["C:/mount/data/"]})),["C:/mount/data/"]);
 assert.ok(isBootPartition(part({name:"Disk 0 partition 1",partition_label:"System",path:"\\\\?\\Volume{efi}\\",filesystem:"FAT32"})));
 assert.ok(!isBootPartition(part()));
 assert.ok(!isBootPartition(part({name:"sda1",kind:"part",path:"/dev/sda1",partition_label:"System"})));
});
test("Windows cards use friendly disk identity while Linux paths and unknown media stay intact",()=>{
 assert.equal(blockDeviceTitle(part({name:"Disk 0",kind:"disk",path:"\\\\?\\scsi#hardware-id"})),"磁盘 0");
 assert.equal(blockDeviceTitle(part()),"磁盘 0 · 分区 3");
 assert.equal(blockDeviceTitle(part({name:"sda",kind:"disk",path:"/dev/sda"})),"/dev/sda");
 const ssd={kind:"disk",model:"Samsung SSD 970 PRO 512GB",rotational:null};
 assert.ok(solidState(ssd));assert.equal(diskRecording(ssd),null);
 assert.ok(!solidState({...ssd,rotational:true}));
 assert.ok(!solidState({...ssd,model:"Generic NVMe device"}));
 assert.equal(windowsMountKey("c:/"),windowsMountKey("C:\\"));
 assert.notEqual(windowsMountKey("C:\\data\\"),windowsMountKey("D:\\data\\"));
});
