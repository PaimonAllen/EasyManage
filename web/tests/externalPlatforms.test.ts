import assert from "node:assert/strict";
import { test } from "node:test";
import { externalPlatformUrl } from "../src/lib/externalPlatforms.ts";
import { saveExternalPlatform,changeExternalPlatform,type ExternalPlatform } from "../src/api/externalPlatforms.ts";

test("external destinations preserve routes and ports but cannot execute scripts or carry URL passwords",()=>{
  assert.equal(externalPlatformUrl(" https://compute.example/gpu?region=lab#login "),"https://compute.example/gpu?region=lab#login");
  assert.equal(externalPlatformUrl("http://192.0.2.1:8080/"),"http://192.0.2.1:8080/");
  for(const url of ["javascript:alert(1)","data:text/html,test","file:///tmp/a","//compute.example","https://user:password@compute.example","https://good\n.example",""]){assert.equal(externalPlatformUrl(url),null);}
});

test("shortcut creation and soft deletion only call the platform API with versioned metadata",async()=>{
  const original=globalThis.fetch;const calls:{url:string;init:RequestInit|undefined}[]=[];
  const fixture:ExternalPlatform={id:"external-id",name:"Compute",url:"https://compute.example/",note:"note",revision:3,deleted_at:null,created_at:"fixture",updated_at:"fixture"};
  try{
    globalThis.fetch=async(url,init)=>{calls.push({url:String(url),init});return new Response(JSON.stringify(fixture),{headers:{"Content-Type":"application/json"}});};
    await saveExternalPlatform({name:fixture.name,url:fixture.url,note:fixture.note});
    await changeExternalPlatform(fixture,"delete");
    assert.equal(calls[0].url,"/api/v1/external-platforms");
    assert.deepEqual(JSON.parse(calls[0].init?.body as string),{name:"Compute",url:fixture.url,note:"note"});
    assert.equal(calls[1].url,"/api/v1/external-platforms/external-id/manage");
    assert.deepEqual(JSON.parse(calls[1].init?.body as string),{action:"delete",confirmation:"external-id",expected_revision:3});
  }finally{globalThis.fetch=original;}
});
