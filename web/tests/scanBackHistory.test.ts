import assert from "node:assert/strict";
import {test} from "node:test";
import {createRequire} from "node:module";
import {installScanBackHistory,SCAN_BACK_STATE} from "../src/lib/scanBackHistory.ts";
const {JSDOM}=createRequire("/tmp/ui-test-deps/package.json")("jsdom");
const wait=async(check:()=>boolean)=>{for(let i=0;i<100;i++){if(check())return;await new Promise(r=>setTimeout(r,5));}assert.ok(check(),"history traversal completed");};
function browser(){
  const dom=new JSDOM("<!doctype html><body></body>",{url:"https://fixture.invalid/#/overview"});
  dom.window.history.replaceState({router:"previous"},"");
  dom.window.history.scrollRestoration="auto";
  return dom;
}
test("Back goes up, cancel stays, confirmed exit skips the guard and cleanup leaves other pages alone",async()=>{
  const dom=browser(),w=dom.window;w.history.pushState(null,"","#/storage");
  let depth=2,dialogs=0,accepted=false;w.confirm=()=>{dialogs++;return accepted;};
  let dispose=installScanBackHistory(leave=>{if(depth)depth--;else leave();},w);
  const length=w.history.length;
  const marker=w.history.state[SCAN_BACK_STATE];
  dispose();dispose=installScanBackHistory(leave=>{if(depth)depth--;else leave();},w);
  assert.equal(w.history.length,length,"StrictMode and remount do not duplicate the guard");
  w.history.back();await wait(()=>depth===1);assert.equal(dialogs,0);
  w.history.back();await wait(()=>depth===0);assert.equal(dialogs,0);
  w.history.back();await wait(()=>dialogs===1);assert.equal(w.location.hash,"#/storage");
  accepted=true;w.history.back();await wait(()=>w.location.hash==="#/overview");
  assert.equal(dialogs,2);assert.deepEqual(w.history.state,{router:"previous"});
  dispose();w.history.forward();await wait(()=>w.location.hash==="#/storage");assert.equal(dialogs,2);
  dom.window.close();

  // A new Document models F5, whose guard must remain in the new document.
  const reload=browser(),next=reload.window;
  next.history.pushState({[SCAN_BACK_STATE]:{...marker,role:"base",distance:1}},"","#/storage");
  next.history.pushState({[SCAN_BACK_STATE]:marker},"");
  let up=0;next.confirm=()=>true;
  const stop=installScanBackHistory(leave=>{if(up===0)up++;else leave();},next);
  assert.equal(next.history.state[SCAN_BACK_STATE].distance,3);
  next.history.back();await wait(()=>up===1);
  next.history.back();await wait(()=>next.location.hash==="#/overview");
  assert.deepEqual(next.history.state,{router:"previous"});stop();reload.window.close();
});
test("a repaired hash traversal never reaches the app router; a first-page exit falls back to overview without warning on reload",async()=>{
  const dom=browser(),w=dom.window;w.history.pushState(null,"","#/storage");
  let depth=1,accept=false;const routes:string[]=[];
  w.addEventListener("hashchange",()=>routes.push(w.location.hash));
  w.confirm=()=>accept;
  const stop=installScanBackHistory(leave=>{if(depth)depth--;else leave();},w);
  w.history.go(-2);await wait(()=>depth===0);await new Promise(r=>setTimeout(r,20));
  assert.equal(w.location.hash,"#/storage");assert.deepEqual(routes,[]);
  accept=true;w.history.back();await wait(()=>w.location.hash==="#/overview");stop();dom.window.close();
  const direct=new JSDOM("<body></body>",{url:"https://fixture.invalid/#/storage"});
  direct.window.confirm=()=>true;
  const dispose=installScanBackHistory(leave=>leave(),direct.window);
  const reloadEvent=new direct.window.Event("beforeunload",{cancelable:true});
  direct.window.dispatchEvent(reloadEvent);assert.equal(reloadEvent.defaultPrevented,false);
  direct.window.history.back();await wait(()=>direct.window.location.hash==="#/overview");
  dispose();direct.window.close();
});

test("BFCache Forward re-arms an accepted leave without immediately exiting again",async()=>{
  const dom=browser(),w=dom.window;w.history.pushState(null,"","#/storage");
  let dialogs=0;w.confirm=()=>{dialogs++;return true;};
  const stop=installScanBackHistory(leave=>leave(),w);
  const guard=w.history.state[SCAN_BACK_STATE],base={...guard,role:"base",distance:guard.distance-1};
  w.history.back();await wait(()=>w.location.hash==="#/overview");assert.equal(dialogs,1);
  // Model pageshow before the restored document's pending popstate.
  w.history.pushState({[SCAN_BACK_STATE]:base},"","#/storage");
  w.dispatchEvent(new w.PageTransitionEvent("pageshow",{persisted:true}));
  w.dispatchEvent(new w.PopStateEvent("popstate",{state:{[SCAN_BACK_STATE]:base}}));
  assert.equal(dialogs,1);assert.equal(w.history.state[SCAN_BACK_STATE].role,"guard");
  w.history.back();await wait(()=>dialogs===2);await wait(()=>w.location.hash==="#/overview");
  stop();dom.window.close();
});
