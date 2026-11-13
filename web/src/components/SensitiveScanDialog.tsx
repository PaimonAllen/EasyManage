import {useEffect,useRef,useState} from "react";
import {AdminCredentialFields,useAdminCredentials} from "./AdminCredentials";
import {reviewSpaceScan,startReviewedSpaceScan,type SpaceScanRequest,type SpaceReview} from "../api/diskSpace";

export function SensitiveScanDialog({nodeId,hostname,platform="linux",request,onClose,onStarted}:{nodeId:string;hostname:string;platform?:string;request:SpaceScanRequest;onClose:()=>void;onStarted:(id:string)=>void}){
  const dialog=useRef<HTMLDialogElement>(null);const mounted=useRef(true);
  const auth=useAdminCredentials([nodeId]);const [review,setReview]=useState<SpaceReview>();
  const [confirmed,setConfirmed]=useState(false);const [busy,setBusy]=useState(false);const [error,setError]=useState("");
  useEffect(()=>{mounted.current=true;dialog.current?.showModal();return()=>{mounted.current=false;dialog.current?.close();auth.clear();};},[]);
  async function verify(){
    if(busy)return;setBusy(true);setError("");
    try{await auth.prepare();const value=await reviewSpaceScan(nodeId,request);if(mounted.current){setReview(value);setConfirmed(false);}}
    catch(e){if(mounted.current)setError(e instanceof Error?e.message:"管理员验证或安全检查失败");}
    finally{if(mounted.current)setBusy(false);}
  }
  async function start(){
    if(busy||!review||!confirmed)return;setBusy(true);setError("");
    try{if(Date.parse(review.expires_at)<=Date.now())throw new Error("安全确认已过期，请重新验证并检查。");
      const job=await startReviewedSpaceScan(nodeId,request,{review_token:review.review_token,confirmed:true});
      if(mounted.current)onStarted(job.id);
    }catch(e){if(mounted.current){setError(e instanceof Error?e.message:"扫描启动失败，请重新检查后确认");setReview(undefined);setConfirmed(false);}}
    finally{if(mounted.current)setBusy(false);}
  }
  return <dialog ref={dialog} className="credentials-dialog" aria-label="敏感范围扫描确认" onCancel={e=>{if(busy)e.preventDefault();else onClose();}}>
    <div className="dialog-title"><h2>{review?"再次确认只读扫描":"管理员确认敏感范围"}</h2><button type="button" className="icon-button" disabled={busy} onClick={onClose} aria-label="关闭">×</button></div>
    <p><strong>{hostname}</strong> · {request.scope==="all_local"?"全部本地磁盘":request.path}</p>
    {!review?<><p className="notice">此范围涉及顶层目录或系统目录。先验证本机管理员身份，再由平台检查固定扫描指令；通过后仍需再次确认才会开始。</p><AdminCredentialFields auth={auth} disabled={busy} platform={platform}/></>:<>
      <p className="notice notice--success">管理员已验证，扫描指令安全检查已通过。尚未启动扫描。</p>
      <p>执行身份：{review.mode==="full"?platform==="windows"?"管理员":"root":"原 SSH 普通用户"} · {review.storage==="realtime"?"实时扫描":"全量快照"}</p>
      <ul>{review.checks.map(check=><li key={check}>{check}</li>)}</ul>
      <p>执行时还会检查：</p><ul>{review.runtime_checks.map(check=><li key={check}>{check}</li>)}</ul>
      <details><summary>只读扫描脚本指纹</summary><code className="sensitive-scan__hash">{review.script_sha256}</code></details>
      <label className="sensitive-scan__confirm"><input type="checkbox" checked={confirmed} disabled={busy} onChange={e=>setConfirmed(e.target.checked)}/>我已核对机器和目录，确认仅执行本次只读扫描。</label>
    </>}
    {error&&<p className="form-error" role="alert">{error}</p>}
    <div className="dialog-actions"><button type="button" className="button button--secondary" disabled={busy} onClick={onClose}>取消</button>
      {review?<><button type="button" className="button button--secondary" disabled={busy} onClick={()=>{setReview(undefined);setConfirmed(false);}}>重新检查</button><button type="button" className="button" disabled={busy||!confirmed} onClick={()=>void start()}>{busy?"正在启动…":"再次确认并开始扫描"}</button></>:<button type="button" className="button" disabled={busy||!auth.ready} onClick={()=>void verify()}>{busy?"正在验证并检查…":"验证管理员并检查安全"}</button>}
    </div>
  </dialog>;
}
