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
import React,{useEffect,useState} from "react";
import {createRoot} from "react-dom/client";
import {ScanBackNavigation} from "/src/components/ScanBackNavigation";
import {PagedSpaceVisualization} from "/src/components/PagedSpaceVisualization";
import "/src/styles.css";
import "/src/components/DiskSpaceExplorer.css";
import {ScanProgress} from "/src/components/ScanProgress";
window.scanCalls=[];
const entry=(id,parent,name)=>({id,parent,name,kind:"directory",allocated:100,apparent:100,files:1,directories:1,issue:null,incomplete:false,mtime:0});
const entries=[entry(0,null,"/fixture"),entry(1,0,"alpha"),entry(2,1,"beta"),entry(3,2,"leaf")];
const result={root:"/fixture",uid:0,username:"root",status:"complete",reason:"",visited:4,errors:0,skipped_mounts:0,symlinks:0,hardlinks:0,nodes:[entries[0]],revision:1};
const original=window.fetch;
window.fetch=async(url,options={})=>{
 if(!String(url).startsWith("/api/"))return original(url,options);
 window.scanCalls.push({url:String(url),method:options.method??"GET"});
 const query=new URL(url,location.origin).searchParams;
 const id=Number(query.get("parent")??0),directory=entries[id]??entries[0],children=entries.filter(e=>e.parent===id);
 const ancestors=[];let current=directory;
 while(current){ancestors.unshift(current);current=current.parent===null?null:entries[current.parent];}
 return new Response(JSON.stringify({directory,entries:children,top:children,ancestors,total:children.length,offset:0,limit:250}),{headers:{"Content-Type":"application/json"}});
};
function Fixture(){
 const [route,setRoute]=useState(location.hash);
 useEffect(()=>{const change=()=>setRoute(location.hash);window.addEventListener("hashchange",change);return()=>window.removeEventListener("hashchange",change);},[]);
 return route==="#/storage"?React.createElement(ScanBackNavigation,null,React.createElement(ScanProgress,{scan:{id:"animation-fixture",status:"running",progress:{estimated_percent:37.4}}}),React.createElement(PagedSpaceVisualization,{nodeId:"browser-node",scanId:"browser-scan",result,onChoosePath:()=>{}})):React.createElement("main",null,React.createElement("h1",{id:"outside"},"设备总览"),React.createElement("a",{id:"enter",href:"#/storage"},"进入存储"));
}
createRoot(document.getElementById("root")).render(React.createElement(React.StrictMode,null,React.createElement(Fixture)));
`;
const server=await createServer({configFile:false,root:fileURLToPath(new URL("..",import.meta.url)),cacheDir:"/tmp/back-browser-vite",
 plugins:[{name:"back-fixture",resolveId(id){if(id==="/back-fixture.tsx")return "\0back-fixture.tsx";},load(id){if(id==="\0back-fixture.tsx")return fixture;},
 configureServer(server){server.middlewares.use(async(req,res,next)=>{if(req.url?.split("?")[0]!=="/back-fixture")return next();res.setHeader("Content-Type","text/html");res.end(await server.transformIndexHtml(req.url,'<!doctype html><html><head></head><body><div id="root"></div><script type="module" src="/back-fixture.tsx"></script></body></html>'));});}},react()],
 server:{host:"127.0.0.1",port:0,watch:null}});
let browser;
try{
 await server.listen();const address=server.httpServer.address();const base="http://127.0.0.1:"+address.port+"/back-fixture";
 browser=await chromium.launch({headless:true,args:["--no-sandbox","--disable-dev-shm-usage","--disable-gpu"]});
 const page=await browser.newPage({viewport:{width:1200,height:900}});
 const errors=[];page.on("pageerror",e=>errors.push(e.message));
 await page.goto(base+"#/overview");await page.locator("#enter").click();
 const progress=await page.locator('.disk-explorer__estimated-progress').evaluate(el=>({value:el.querySelector('progress').value,max:el.querySelector('progress').max,text:el.textContent,dots:[...el.querySelectorAll('i')].map(dot=>getComputedStyle(dot).animationName)}));
 assert.equal(progress.value,37.4);assert.equal(progress.max,100);assert.ok(progress.text.includes('预计 37.4%'));
 assert.equal(progress.dots.length,5);assert.ok(progress.dots.every(name=>name==='disk-scan-dots'));
 await page.emulateMedia({reducedMotion:'reduce'});
 assert.equal(await page.locator('.disk-explorer__loading-dots i').first().evaluate(el=>getComputedStyle(el).animationName),'none');
 await page.emulateMedia({reducedMotion:'no-preference'});
 await page.locator(".disk-explorer__name").filter({hasText:"alpha"}).click();
 assert.equal(await page.locator(".disk-explorer__breadcrumbs button").last().innerText(),"/fixture","single click stays in the current directory");
 await page.locator(".disk-explorer__name").filter({hasText:"alpha"}).dblclick();
 await page.locator(".disk-explorer__name").filter({hasText:"beta"}).dblclick();
 await page.locator(".disk-explorer__name").filter({hasText:"leaf"}).waitFor();
 await page.evaluate(()=>history.back());
 await page.locator(".disk-explorer__breadcrumbs button").filter({hasText:"alpha"}).waitFor();
 assert.match(page.url(),/#\/storage$/);
 assert.equal(await page.locator(".disk-explorer__breadcrumbs button").last().innerText(),"alpha");
 await page.reload();await page.locator(".disk-explorer__name").filter({hasText:"beta"}).waitFor();
 assert.equal(await page.locator(".disk-explorer__breadcrumbs button").last().innerText(),"alpha");
 await page.locator(".disk-explorer__filter input").click();
 await page.evaluate(()=>history.back());
 await page.locator(".disk-explorer__name").filter({hasText:"alpha"}).waitFor();
 assert.equal(await page.locator(".disk-explorer__breadcrumbs button").last().innerText(),"/fixture");
 let dialogs=0,accept=false;
 page.on("dialog",async dialog=>{dialogs++;assert.match(dialog.message(),/是否离开当前页面/);if(accept)await dialog.accept();else await dialog.dismiss();});
 await page.evaluate(()=>history.back());for(let i=0;i<100&&dialogs<1;i++)await page.waitForTimeout(20);
 assert.equal(dialogs,1);assert.match(page.url(),/#\/storage$/);
 assert.equal(await page.evaluate(()=>window.scanCalls.some(call=>call.method!=="GET")),false);
 accept=true;await page.evaluate(()=>history.back());await page.locator("#outside").waitFor();
 assert.equal(dialogs,2);assert.match(page.url(),/#\/overview$/);
 assert.deepEqual(errors,[]);
 console.log("PASS real Chromium directory Back, StrictMode, F5 recovery, cancel/confirm native dialog, original route exit and no scan mutations");
}finally{await browser?.close();await server.close();}
