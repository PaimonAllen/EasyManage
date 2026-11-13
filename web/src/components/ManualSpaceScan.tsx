import {ScanProgress} from "./ScanProgress";
import {useEffect,useRef,useState,type ReactNode} from "react";
import {cancelSpaceScan,fetchSpaceScan,keepSpaceResults,pauseSpaceScan,releaseSpaceResults,type SpaceScan} from "../api/diskSpace";

const labels:Record<string,string>={complete:"完成",partial:"部分完成",cancelled:"已取消",failed:"失败",interrupted:"已中断"};

export function ManualSpaceScan({nodeId,scanId,path,starting,error,onStart,onDetail,children}:{
  nodeId:string;scanId?:string;path:string;starting:boolean;error?:string;onStart:()=>void;onDetail:(scan:SpaceScan)=>void;children:(scan:SpaceScan)=>ReactNode;
}){
  const [scan,setScan]=useState<SpaceScan|null>(null);const [failure,setFailure]=useState("");const [refresh,setRefresh]=useState(0);const [busy,setBusy]=useState(false);
  const [viewer]=useState(()=>crypto.randomUUID());const detail=useRef(onDetail);detail.current=onDetail;
  useEffect(()=>{
    const controller=new AbortController();
    const attach=()=>{void keepSpaceResults(nodeId,viewer,controller.signal).catch(()=>{});};
    const detach=()=>releaseSpaceResults(nodeId,viewer);
    attach();const timer=setInterval(attach,30_000);window.addEventListener("pagehide",detach);
    return()=>{clearInterval(timer);controller.abort();window.removeEventListener("pagehide",detach);detach();};
  },[nodeId,viewer]);
  useEffect(()=>{
    if(!scanId){setScan(null);setFailure("");return;}
    const controller=new AbortController();let timer:ReturnType<typeof setTimeout>;
    const load=async()=>{
      try{
        const next=await fetchSpaceScan(nodeId,scanId,controller.signal);
        if(controller.signal.aborted)return;
        setScan(next);setFailure("");detail.current(next);
        timer=setTimeout(load,next.status==="running"?1000:60000);
      }catch(e){if(!controller.signal.aborted){setFailure(e instanceof Error?e.message:"目标扫描读取失败");timer=setTimeout(load,5000);}}
    };
    void load();return()=>{controller.abort();clearTimeout(timer);};
  },[nodeId,scanId,refresh]);
  async function control(cancel=false){
    if(!scan)return;setBusy(true);setFailure("");
    try{if(cancel)await cancelSpaceScan(nodeId,scan.id);else await pauseSpaceScan(nodeId,scan.id,!scan.paused);setRefresh(v=>v+1);}
    catch(e){setFailure(e instanceof Error?e.message:"目标扫描控制失败");}finally{setBusy(false);}
  }
  const running=scan?.status==="running";
  return <section className="disk-explorer__linked" aria-label={"手动目标扫描 "+path}>
    <header><strong>{path}</strong><span role="status">{starting?"正在启动目标扫描…":scan?.paused?"目标扫描已暂停":running?"目标扫描中":scan?"目标扫描"+(labels[scan.status]??scan.status):"等待手动扫描"}</span>
      {scan&&<ScanProgress key={scan.id} scan={scan}/>}
      {running?<>{scan.pause_supported&&<button className="button button--secondary button--small" disabled={busy||scan.pause_pending} onClick={()=>void control()}>{scan.pause_pending?"等待确认…":scan.paused?"继续目标扫描":"暂停目标扫描"}</button>}<button className="button button--secondary button--small" disabled={busy} onClick={()=>void control(true)}>取消目标扫描</button></>:
      <button className="button button--secondary button--small" disabled={starting} onClick={onStart}>{scanId?"重新扫描目标":"展开扫描"}</button>}
    </header>
    <p className="hint">目标采用独立实时扫描，无人查看后保留 24 小时，不并入上级目录统计。收起后任务继续，刷新可恢复展开。</p>
    {(error||failure||scan?.message)&&<p className="form-error" role="alert">{error||failure||scan?.message}</p>}
    {scan?.result?children(scan):scanId&&!failure&&<p className="hint" role="status">正在读取目标目录…</p>}
  </section>;
}
