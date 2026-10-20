import { useEffect, useRef, useState } from "react";
import { fetchCollectionSettings, savePermissionCollection, saveUserCollection, saveProxyCollection, type CollectionSettingsStatus } from "../api/client";

export function NodeCollectionToggle({hostname,label,enabled,applied,online,busy,onChange,unavailable=false}: {
  hostname:string; label:string; enabled:boolean|undefined; applied:boolean|undefined;
  online:boolean; busy:boolean; onChange:()=>void;
  unavailable?:boolean;
}) {
  const confirmed=enabled!==undefined && applied===enabled;
  return <div className="user-collection-control">
    <label><input type="checkbox" role="switch" aria-label={`${hostname} ${label}`}
      checked={enabled??false} disabled={enabled===undefined||busy} onChange={onChange}/>
      {label} · {unavailable?"平台需更新":enabled===undefined?"读取中…":enabled?"开启":"关闭"}</label>
    <small className="muted">{unavailable?"当前平台后端不支持此开关，请先更新平台":busy?"保存中…":enabled===undefined?"尚未读取开关状态":!online?"离线 · 上线后应用":applied===undefined?"需更新 Agent 才能控制采集":confirmed?enabled?"Agent 已开启采集":"Agent 已停止采集":"已保存 · 等待 Agent 确认"}</small>
  </div>;
}

export function useNodeCollection(kind:"users"|"permissions"|"proxy") {
  const [settings,setSettings]=useState<CollectionSettingsStatus|null>(null);
  const [settingsError,setSettingsError]=useState("");
  const [busy,setBusy]=useState<Record<string,boolean>>({});
  const [errors,setErrors]=useState<Record<string,string>>({});
  const revision=useRef(0);
  const active=useRef(false);
  const saving=useRef(new Set<string>());
  useEffect(()=>{
    active.current=true;
    const controller=new AbortController(); let pending=false;
    async function refresh(){
      if(pending||document.hidden||saving.current.size)return;
      pending=true; const started=revision.current;
      try { const next=await fetchCollectionSettings(AbortSignal.any([controller.signal,AbortSignal.timeout(10_000)]));
        if(!controller.signal.aborted && revision.current===started){setSettings(next);setSettingsError("");} }
      catch(error){if(!controller.signal.aborted&&revision.current===started)setSettingsError(error instanceof Error?error.message:"采集开关读取失败");}
      finally {pending=false;}
    }
    void refresh(); const timer=window.setInterval(()=>void refresh(),15_000);
    document.addEventListener("visibilitychange",refresh);
    return()=>{active.current=false;controller.abort();window.clearInterval(timer);document.removeEventListener("visibilitychange",refresh);};
  },[]);
  async function toggle(id:string,enabled:boolean){
    if(saving.current.has(id))return;
    saving.current.add(id);revision.current+=1;
    setBusy(previous=>({...previous,[id]:true}));setErrors(previous=>({...previous,[id]:""}));
    try{
      const save={users:saveUserCollection,permissions:savePermissionCollection,proxy:saveProxyCollection}[kind];
      const result=await save(id,!enabled,enabled);
      if(active.current)setSettings(previous=>previous?{...previous,agents:previous.agents.map(agent=>agent.node_id===id?{...agent,[`${kind}_enabled`]:result.enabled}:agent)}:previous);
    }catch(error){
      if(active.current)setErrors(previous=>({...previous,[id]:error instanceof Error?error.message:"采集开关保存失败"}));
      // Reconcile conflicts on the next poll without overwriting another in-flight save.
    }finally{
      revision.current+=1;saving.current.delete(id);
      if(active.current)setBusy(previous=>({...previous,[id]:false}));
    }
  }
  return {settings,settingsError,busy,errors,toggle};
}
