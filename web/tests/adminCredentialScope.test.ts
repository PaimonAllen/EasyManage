import assert from "node:assert/strict";
import {test} from "node:test";
import {AdminCredentialScopeError, verifyAdminCredentialScopes} from "../src/lib/adminCredentialScope.ts";

test("a batch verifies each selected machine's saved browser authorization before returning",async()=>{
  const calls:string[]=[];
  await verifyAdminCredentialScopes(["first","second","first"],
    async id=>{calls.push(`save:${id}`);},async id=>{calls.push(`status:${id}`);return {available:true};});
  assert.deepEqual(calls,["save:first","status:first","save:second","status:second"]);
});

test("partial authorization failure identifies the machine and prevents batch preparation",async()=>{
  const calls:string[]=[];
  await assert.rejects(verifyAdminCredentialScopes(["first","second","third"],
    async id=>{calls.push(`save:${id}`);if(id==="second")throw new Error("saved authorization expired");},
    async id=>{calls.push(`status:${id}`);return {available:true};}),
    error=>error instanceof AdminCredentialScopeError&&error.nodeId==="second"&&error.message==="saved authorization expired");
  assert.deepEqual(calls,["save:first","status:first","save:second"]);
});

test("successful SSH/sudo with a rejected cookie is distinct from a wrong password",async()=>{
  let saved=false;
  await assert.rejects(verifyAdminCredentialScopes(["first"],async()=>{saved=true;},async()=>({available:false})),
    error=>error instanceof AdminCredentialScopeError&&error.nodeId==="first"&&error.message.includes("Cookie")&&error.message.includes("未开始更新"));
  assert.equal(saved,true);
});

test("platform administrator authorization remains separate from machine scopes",async()=>{
  const scopes:(string|null)[]=[];
  await verifyAdminCredentialScopes([null],async id=>{scopes.push(id);},async()=>({available:true}));
  assert.deepEqual(scopes,[null]);
});
