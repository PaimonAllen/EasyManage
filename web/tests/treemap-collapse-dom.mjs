// Run only in the documented private bwrap namespaces with test-only jsdom.
import assert from "node:assert/strict";
import {readlink,readFile} from "node:fs/promises";
import {createRequire} from "node:module";
import {fileURLToPath} from "node:url";
import React,{act} from "react";
import {createServer} from "vite";
import react from "@vitejs/plugin-react";
const [hostNet,hostMount,deps="/tmp/ui-test-deps"]=process.argv.slice(2);
assert.ok(hostNet&&hostMount);
assert.notEqual(await readlink('/proc/self/ns/net'),hostNet);
assert.notEqual(await readlink('/proc/self/ns/mnt'),hostMount);
assert.ok((await readFile('/proc/self/mountinfo','utf8')).split('\n').some(line=>line.split(' ')[4]==='/tmp'&&line.includes(' - tmpfs ')));
const {JSDOM}=createRequire(deps+'/package.json')('jsdom');
const dom=new JSDOM('<!doctype html><div id="root"></div>',{url:'https://fixture.invalid/'});
globalThis.window=dom.window;globalThis.document=dom.window.document;
Object.defineProperty(globalThis,'navigator',{value:dom.window.navigator,configurable:true});
globalThis.HTMLElement=dom.window.HTMLElement;globalThis.IS_REACT_ACT_ENVIRONMENT=true;
const {createRoot}=await import('react-dom/client');

const server=await createServer({configFile:false,root:fileURLToPath(new URL('..',import.meta.url)),plugins:[react()],cacheDir:'/tmp/disk-live-vite',server:{middlewareMode:true,watch:null}});
const root=createRoot(document.querySelector('#root'));

const top={id:0,parent:null,name:'/fixture',kind:'directory',allocated:100,apparent:100,files:1,directories:1,issue:null,incomplete:false,mtime:0};
const child={...top,id:1,parent:0,name:'alpha',allocated:80,directories:0};
const result={root:'/fixture',uid:0,username:'root',status:'complete',reason:'',visited:2,elapsed_seconds:1,errors:0,skipped_mounts:0,symlinks:0,hardlinks:0,nodes:[top,child],revision:1};
const requests=[];
let size=80;
globalThis.fetch=async(url,options)=>{
  requests.push({url,options});
  return new Response(JSON.stringify({directory:top,ancestors:[top],entries:[{...child,allocated:size}],top:[{...child,allocated:size}],total:1,offset:0,limit:250}),{headers:{'Content-Type':'application/json'}});
};
const frame=async(fn)=>act(async()=>{fn();await new Promise(r=>setTimeout(r,0));});
const button=text=>[...document.querySelectorAll('button')].find(b=>b.textContent===text);
try{
  const {PagedSpaceVisualization}=await server.ssrLoadModule('/src/components/PagedSpaceVisualization.tsx');
  const {SpaceVisualization}=await server.ssrLoadModule('/src/components/DiskSpaceExplorer.tsx');
  for(const [Component,label] of [[PagedSpaceVisualization,'paged'],[SpaceVisualization,'snapshot']]){
    const props={nodeId:'collapse-node',scanId:label,result,onChoosePath:()=>{}};
    await frame(()=>root.render(React.createElement(Component,{...props,key:label})));
    const table=document.querySelector('table');assert.ok(table);
    const collapse=button('折叠本图');assert.ok(collapse);assert.equal(collapse.getAttribute('aria-expanded'),'true');
    const target=document.getElementById(collapse.getAttribute('aria-controls'));assert.ok(target);assert.equal(target.hidden,false);
    assert.ok(target.querySelector('svg'));
    const before=requests.length;
    await frame(()=>collapse.click());
    assert.equal(target.hidden,true);assert.equal(button('展开本图').getAttribute('aria-expanded'),'false');
    assert.equal(document.querySelector('table'),table);assert.equal(requests.length,before,'folding does not fetch or control any scan');
    size=50;
    await frame(()=>root.render(React.createElement(Component,{...props,key:label,result:{...result,revision:2}})));
    assert.equal(target.hidden,true,'result updates do not reopen the chart');
    assert.equal(document.querySelector('table'),table,'live values preserve the directory table');
    if(label==='paged')assert.ok(table.textContent.includes('50 B'),'directory values keep refreshing while chart is collapsed');
    await frame(()=>root.render(React.createElement(Component,{...props,key:label+'-f5',result:{...result,revision:2}})));
    const expand=button('展开本图');assert.ok(expand,'F5 restores the saved collapsed choice');
    assert.equal(document.getElementById(expand.getAttribute('aria-controls')).hidden,true);
    await frame(()=>expand.click());
    assert.ok(button('折叠本图'));assert.ok(document.querySelector('.disk-explorer__treemap'));
    assert.equal(document.getElementById(button('折叠本图').getAttribute('aria-controls')).hidden,false);
    assert.ok(requests.every(r=>!r.options||r.options.method!=='POST'),'chart controls never start, pause or cancel a scan');
    console.log('PASS '+label+' chart collapse/expand, accessible state, unchanged table, background refresh and F5 restoration');
  }
}finally{await frame(()=>root.unmount());await server.close();dom.window.close();}
