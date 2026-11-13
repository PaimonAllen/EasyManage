import assert from "node:assert/strict";
import {test} from "node:test";
import {sensitiveScanPath} from "../src/lib/scanSafety.ts";
test("sensitive scope UI matches root/system boundaries while user data stays public",()=>{
 for(const path of ["/","//","/home","/data","/root/private","/etc/ssh","/var/lib/postgresql","/usr/local"])assert.equal(sensitiveScanPath(path,"directory"),true,path);
 for(const path of ["/home/zbh","/home/zbh/bigdata","/data/data_hdd_3","/tmp/fixture","/etc-backup/data"])assert.equal(sensitiveScanPath(path,"directory"),false,path);
 for(const path of ["C:/","C:/Users","C:/Windows/System32","C:/ProgramData/data","C:\\Windows\\System32"])assert.equal(sensitiveScanPath(path,"directory",true),true,path);
 assert.equal(sensitiveScanPath("C:/Users/person/data","directory",true),false);
 assert.equal(sensitiveScanPath("/home/zbh","all_local"),true);
});
