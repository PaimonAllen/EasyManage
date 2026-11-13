import assert from "node:assert/strict";
import {test} from "node:test";
import {isFullPrivilege,privilegeIdentity,canDeployPlatform} from "../src/lib/hostPlatform.ts";
test("Windows uses the native token and SID rather than the legacy UID field",()=>{
 const full={mode:"full",username:"SYSTEM",uid:4294967295,native:{platform:"windows",principal:"S-1-5-18",administrator:true,capabilities:[]}};
 assert.equal(isFullPrivilege(full),true);assert.equal(privilegeIdentity(full),"SID S-1-5-18");
 assert.equal(isFullPrivilege({...full,uid:0,native:{...full.native,administrator:false}}),false);
 assert.equal(isFullPrivilege({...full,mode:"basic"}),false);
 assert.equal(isFullPrivilege({mode:"full",username:"root",uid:0}),true);
 assert.equal(isFullPrivilege({mode:"full",username:"user",uid:1000}),false);
});

test("candidate readiness follows the selected operating system",()=>{
 const candidates={linux:true,windows:false};
 assert.equal(canDeployPlatform("windows",true,candidates),false);
 assert.equal(canDeployPlatform("linux",true,candidates),true);
 assert.equal(canDeployPlatform("windows",false,{linux:true,windows:true}),false);
 assert.equal(canDeployPlatform("windows",true),true);
});
