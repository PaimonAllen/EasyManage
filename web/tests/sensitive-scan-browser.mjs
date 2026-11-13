// Real Chromium inside private bwrap. All API data is synthetic and in-browser.
import assert from "node:assert/strict";
import {readlink} from "node:fs/promises";
import {createRequire} from "node:module";
import {fileURLToPath} from "node:url";
import {createServer} from "vite";
import react from "@vitejs/plugin-react";
const [hostNet,hostMount]=process.argv.slice(2);
assert.notEqual(await readlink("/proc/self/ns/net"),hostNet);
assert.notEqual(await readlink("/proc/self/ns/mnt"),hostMount);
const {chromium}=createRequire("/tmp/browser-deps/package.json")("playwright");
const fixture=String.raw`
import React,{useState} from "react";
import {createRoot} from "react-dom/client";
import {SensitiveScanDialog} from "/src/components/SensitiveScanDialog";
import "/src/styles.css";
import "/src/components/DiskSpaceExplorer.css";
window.calls=[];
window.fetch=async(url,options={})=>{
 const body=options.body?JSON.parse(options.body):null;window.calls.push({url,method:options.method??"GET",body});
 let data;
 if(String(url).endsWith('/admin-credentials'))data={available:true};
 else if(String(url).endsWith('/review'))data={review_token:'fixture-approval',expires_at:new Date(Date.now()+300000).toISOString(),path:'/',scope:'all_local',mode:'full',storage:'realtime',read_only:true,script_sha256:'a'.repeat(64),checks:['已验证管理员','固定脚本与参数检查通过'],runtime_checks:['不修改源文件']};
 else if(String(url).endsWith('/disk-space'))data={id:'approved-scan'};
 else throw new Error('unexpected request');
 return new Response(JSON.stringify(data),{headers:{'Content-Type':'application/json'}});
};
function Fixture(){const [open,setOpen]=useState(false);const [started,setStarted]=useState('');
 return React.createElement('main',null,React.createElement('button',{id:'open',onClick:()=>setOpen(true)},'扫描根目录'),React.createElement('p',{id:'started'},started),open&&React.createElement(SensitiveScanDialog,{nodeId:'node-a',hostname:'fixture-host',request:{path:'/',mode:'full',seconds:0,max_entries:1,scope:'all_local',storage:'realtime'},onClose:()=>setOpen(false),onStarted:id=>{setStarted(id);setOpen(false);}}));
}
createRoot(document.getElementById('root')).render(React.createElement(Fixture));
`;
const server=await createServer({configFile:false,root:fileURLToPath(new URL('..',import.meta.url)),cacheDir:'/tmp/sensitive-scan-vite',plugins:[{name:'sensitive-fixture',resolveId(id){if(id==='/sensitive-fixture.tsx')return '\0sensitive-fixture.tsx';},load(id){if(id==='\0sensitive-fixture.tsx')return fixture;},configureServer(server){server.middlewares.use(async(req,res,next)=>{if(req.url!=='/sensitive-fixture')return next();res.setHeader('Content-Type','text/html');res.end(await server.transformIndexHtml(req.url,'<!doctype html><html><body><div id="root"></div><script type="module" src="/sensitive-fixture.tsx"></script></body></html>'));});}},react()],server:{host:'127.0.0.1',port:0,watch:null}});
let browser;
try{
 await server.listen();browser=await chromium.launch({headless:true,args:['--no-sandbox','--disable-dev-shm-usage']});
 const page=await browser.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.goto('http://127.0.0.1:'+server.httpServer.address().port+'/sensitive-fixture');
 await page.locator('#open').click();await page.getByRole('heading',{name:'管理员确认敏感范围'}).waitFor();
 assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.url.endsWith('/disk-space')).length),0);
 await page.getByRole('button',{name:'验证管理员并检查安全'}).click();
 await page.getByRole('heading',{name:'再次确认只读扫描'}).waitFor();
 assert.deepEqual(await page.evaluate(()=>window.calls.filter(c=>c.method==='POST').map(c=>c.url.split('/').pop())),['admin-credentials','review']);
 assert.equal(await page.getByRole('button',{name:'再次确认并开始扫描'}).isDisabled(),true);
 await page.getByRole('button',{name:'取消',exact:true}).click();
 assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.url.endsWith('/disk-space')).length),0,'cancelling after review never starts a scan');
 await page.locator('#open').click();await page.getByRole('button',{name:'验证管理员并检查安全'}).click();await page.getByRole('heading',{name:'再次确认只读扫描'}).waitFor();
 await page.getByRole('checkbox').check();await page.getByRole('button',{name:'再次确认并开始扫描'}).click();
 await page.waitForFunction(()=>document.querySelector('#started').textContent==='approved-scan');
 const starts=await page.evaluate(()=>window.calls.filter(c=>c.url.endsWith('/disk-space')));
 assert.equal(starts.length,1);assert.equal(starts[0].body.review_token,'fixture-approval');assert.equal(starts[0].body.confirmed,true);assert.equal(starts[0].body.path,'/');
 assert.equal(errors.length,0,errors.join('\n'));
 console.log('PASS real Chromium administrator-first, backend-review-second, explicit final confirmation, cancellation without start and exact approval request');
}finally{await browser?.close();await server.close();}
