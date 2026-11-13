// One same-document guard keeps Back inside the directory explorer. Markers
// survive refresh; distance skips all prior refresh guards on confirmed exit.
export const SCAN_BACK_STATE="easymanage.scan-back.v1";
interface Marker {version:1;document:string;role:"base"|"guard";distance:number;previous:boolean;url:string}
const documents=new WeakMap<Document,string>();
function marker(state:unknown,url:string):Marker|undefined{
  const value=(state as Record<string,unknown>|null)?.[SCAN_BACK_STATE] as Marker|undefined;
  if(value?.version===1&&value.url===url&&["base","guard"].includes(value.role)&&typeof value.document==="string"
    &&Number.isSafeInteger(value.distance)&&value.distance>0&&typeof value.previous==="boolean")return value;
}
export function installScanBackHistory(onBack:(leave:()=>void)=>void,win:Window=window):()=>void{
  const url=win.location.href;const history=win.history;
  const token=documents.get(win.document)??crypto.randomUUID();documents.set(win.document,token);
  const current=marker(history.state,url);
  const original=history.state&&typeof history.state==="object"?{...history.state}:{};
  delete original[SCAN_BACK_STATE];
  let guard:Marker;let leaving=false;let live=true;let swallowHash=false;let restoredPop=false;let restoreTimer:ReturnType<typeof setTimeout>|undefined;
  let timer:ReturnType<typeof setTimeout>|undefined;let scroll:ScrollRestoration|undefined;
  const write=(value:Marker,replace=false)=>history[replace?"replaceState":"pushState"]({...original,[SCAN_BACK_STATE]:value},"",url);
  if(current?.role==="guard"&&current.document===token){guard=current;}
  else{
    const base:Marker={version:1,document:token,role:"base",distance:current?.distance??1,previous:current?.previous??history.length>1,url};
    write(base,true);guard={...base,role:"guard",distance:base.distance+1};write(guard);
  }
  const restoreScroll=()=>{
    if(scroll!==undefined&&win.location.href===url)history.scrollRestoration=scroll;
    scroll=undefined;timer=undefined;
  };
  const holdScroll=()=>{
    scroll??=history.scrollRestoration;
    history.scrollRestoration="manual";
    if(timer!==undefined)clearTimeout(timer);
    timer=setTimeout(restoreScroll,0);
  };
  const leave=()=>{
    if(!live||leaving)return;
    if(!win.confirm("已在扫描根目录，是否离开当前页面？扫描任务仍会在后台继续。"))return;
    leaving=true;
    if(guard.previous)history.go(-guard.distance);
    else win.location.hash="#/overview";
  };
  const pop=(event:PopStateEvent)=>{
    if(!live||leaving)return;
    if(restoredPop){restoredPop=false;return;}
    const target=marker(event.state,url);
    if(target?.role==="guard"&&target.document===token&&win.location.href===url)return;
    event.stopImmediatePropagation();
    holdScroll();
    const same=win.location.href===url;
    if(!same)swallowHash=true;
    guard={version:1,document:token,role:"guard",distance:same&&target?target.distance+1:1,previous:same&&target?target.previous:true,url};
    write(guard);
    onBack(leave);
  };
  const hash=(event:HashChangeEvent)=>{
    if(!swallowHash)return;
    swallowHash=false;
    // A repaired hash traversal must not trigger App's route change/scroll reset.
    if(win.location.href===url)event.stopImmediatePropagation();
  };
  const show=(event:PageTransitionEvent)=>{
    if(!event.persisted||!live||win.location.href!==url)return;
    // A document restored from BFCache keeps its JS state, including the old
    // leave flag. Re-arm without treating Forward as a request to go up.
    leaving=false;
    const restored=marker(history.state,url);
    if(restored?.role==="base"){
      guard={...restored,document:token,role:"guard",distance:restored.distance+1};write(guard);
      restoredPop=true;
      restoreTimer=setTimeout(()=>{restoredPop=false;},0);
    }
  };
  win.addEventListener("popstate",pop,true);win.addEventListener("hashchange",hash,true);win.addEventListener("pageshow",show);
  return()=>{live=false;win.removeEventListener("popstate",pop,true);win.removeEventListener("hashchange",hash,true);win.removeEventListener("pageshow",show);if(restoreTimer!==undefined)clearTimeout(restoreTimer);if(timer!==undefined)clearTimeout(timer);restoreScroll();};
}
