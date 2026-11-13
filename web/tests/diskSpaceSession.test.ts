import assert from "node:assert/strict";
import {test} from "node:test";
import {readLinkedScans,rememberLinkedScan,readSpaceSession,rememberSpaceNode,readMachineSession,rememberMachineSession,touchSpaceSession,readSpaceView,rememberSpaceView,SESSION_TTL} from "../src/lib/diskSpaceSession.ts";
test("browser sessions keep client identity, machine/task choices and directory pages for 24 idle hours",()=>{
  const store=new Map<string,string>();
  const storage={getItem:(k:string)=>store.get(k)??null,setItem:(k:string,v:string)=>store.set(k,v),removeItem:(k:string)=>store.delete(k)};
  Object.defineProperty(globalThis,"window",{value:{localStorage:storage},configurable:true});
  try{
    assert.equal(readSpaceSession(),null);rememberSpaceNode("node-b");
    const first=readSpaceSession()!;assert.equal(first.node,"node-b");
    assert.ok(first.expires>=Date.now()+SESSION_TTL-1000);
    rememberMachineSession("node-b",{storage:"realtime",scans:{realtime:"my-scan",snapshot:"saved-snapshot"},path:"/home/tzr",customPath:false,scope:"directory",diskKey:"",seconds:0,maxEntries:0});
    rememberSpaceView("node-b","my-scan",{directory:42,basis:"apparent",sort:"name",ascending:true,search:"abc",offset:250,expanded:[45]});
    rememberLinkedScan("node-b","my-scan",5,{id:"child-scan",path:"/home/tzr/bigdata"});
    assert.equal(readLinkedScans("node-b","my-scan")["5"].id,"child-scan");
    assert.deepEqual(readLinkedScans("node-b","other-scan"),{});
    touchSpaceSession();assert.equal(readSpaceSession()!.client,first.client);
    assert.equal(readMachineSession("node-b")!.scans.realtime,"my-scan");
    assert.equal(readSpaceView("node-b","my-scan")!.offset,250);
    assert.equal(readSpaceView("node-b","other-client-scan"),undefined);
    const key=[...store.keys()][0],persisted=store.get(key)!;
    store.set(key,JSON.stringify({...JSON.parse(persisted),expires:Date.now()-1}));
    assert.equal(readSpaceSession(),null);assert.equal(readMachineSession("node-b"),undefined);assert.deepEqual(readLinkedScans("node-b","my-scan"),{});
    rememberSpaceNode("fresh");assert.notEqual(readSpaceSession()!.client,first.client);
    store.set(key,persisted);assert.equal(readSpaceSession()!.client,first.client);
    assert.equal(readSpaceView("node-b","my-scan")!.directory,42);
    store.set(key,"broken JSON");assert.equal(readSpaceSession(),null);
    Object.defineProperty(window,"localStorage",{get(){throw new Error("disabled");}});
    assert.doesNotThrow(touchSpaceSession);assert.equal(readSpaceSession(),null);
  }finally{Reflect.deleteProperty(globalThis,"window");}
});
