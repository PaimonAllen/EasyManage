import assert from "node:assert/strict";
import {test} from "node:test";
import {entryPath,parentPercent,sortedEntries,spaceTreemap,spaceDirectoryPath,excludedSpaceEntry,directorySummaryItems} from "../src/lib/diskSpace.ts";
import type {SpaceEntry} from "../src/api/diskSpace.ts";
const entry=(id:number,parent:number|null,name:string,size:number):SpaceEntry=>({id,parent,name,allocated:size,apparent:size,kind:"directory",files:0,directories:0,issue:null,incomplete:false,mtime:0});
const nodes=[entry(0,null,"/",100),entry(1,0,"home",80),entry(2,1,"nested",40),entry(3,0,"var",20)];
test("hierarchy uses actual parent denominator and retains incomplete lower bounds",()=>{
  assert.equal(parentPercent(nodes[2],nodes,"allocated"),50);
  assert.equal(parentPercent(nodes[1],nodes,"allocated"),80);
  assert.equal(entryPath(nodes[2],nodes),"/home/nested");
  assert.equal(parentPercent({...nodes[0],allocated:0},nodes,"allocated"),null);
  assert.equal(parentPercent({...nodes[2],incomplete:true},nodes,"allocated"),50);
  assert.deepEqual(sortedEntries([nodes[3],nodes[1]],nodes,"allocated","percent",false).map(n=>n.id),[1,3]);
});
test("directory-only treemap separates direct files from unshown child directories",()=>{
  const page={directory:nodes[0],ancestors:[nodes[0]],entries:[nodes[1]],top:[nodes[1]],
    total:45,offset:0,limit:250,direct:{allocated:15,apparent:15,files:7}};
  const items=directorySummaryItems(page,"allocated");
  assert.deepEqual(items.map(i=>[i.name,i.value]),[["home",80],["本层文件及目录自身",15],["其他子目录",5]]);
  assert.equal(items.reduce((n,i)=>n+i.value,0),page.directory.allocated);
  const leaf=directorySummaryItems({...page,top:[],direct:{allocated:100,apparent:100,files:7}},"allocated");
  assert.deepEqual(leaf.map(i=>i.name),["本层文件及目录自身"]);
});
test("treemap area is proportional and bounded; zero/invalid entries excluded",()=>{
  const rectangles=spaceTreemap([{id:1,name:"a",value:80},{id:2,name:"b",value:20},{id:3,name:"zero",value:0}]);
  assert.equal(rectangles.length,2);
  assert.equal(rectangles.reduce((sum,r)=>sum+r.width*r.height,0),300000);
  assert.ok(Math.abs(rectangles[0].width*rectangles[0].height/300000-.8)<1e-8);
  assert.ok(rectangles.every(r=>r.x>=0&&r.y>=0&&r.x+r.width<=1000&&r.y+r.height<=300));
});
test("all-local grouping preserves real paths and marks excluded mounts",()=>{
  const all=[entry(0,null,"/",100),{...entry(1,0,"/data/disk",80),scan_path:"/data/disk"},entry(2,1,"users",40)];
  assert.equal(spaceDirectoryPath(all),"/data/disk/users");
  assert.equal(entryPath(all[2],all),"/data/disk/users");
  assert.equal(spaceDirectoryPath([all[0],{...all[1],scan_path:"/",name:"/"},all[2]]),"/users");
  for(const issue of ['mount_boundary','duplicate_mount','separate_filesystem'])assert.equal(excludedSpaceEntry({...all[2],issue}),true);
  assert.equal(excludedSpaceEntry(all[2]),false);
  assert.equal(parentPercent(all[2],all,'allocated'),50);
});
