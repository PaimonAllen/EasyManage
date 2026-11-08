import {useEffect,useRef,useState} from "react";
import {enableProxyProfile,setProxyMonitoring,setLocalNetworkMonitoring,type ProxyLabStatus,type ProxyMonitoringSettings,type ProxyProfile} from "../api/proxyLab";

export type MonitoringChange={monitoring?:ProxyMonitoringSettings;profile?:{id:string;enabled:boolean}};
export function MonitoringSwitch({label,enabled,disabled,onClick}:{label:string;enabled:boolean;disabled?:boolean;onClick:()=>void}){
  return <button type="button"role="switch"aria-label={label}aria-checked={enabled}disabled={disabled}onClick={onClick}className="button button--small button--secondary proxy-monitoring__switch"><span className="proxy-monitoring__track"aria-hidden="true"/>{label}：{enabled?"开":"关"}</button>;
}
export function MonitoringConfirmation({enabled,busy,error,onConfirm,onCancel}:{enabled:boolean;busy:boolean;error:string;onConfirm:()=>void;onCancel:()=>void}){
  const dialog=useRef<HTMLDialogElement>(null);
  useEffect(()=>{dialog.current?.showModal();},[]);
  return <dialog ref={dialog}className="credentials-dialog"onCancel={e=>{if(busy)e.preventDefault();else onCancel();}}><form onSubmit={e=>{e.preventDefault();onConfirm();}}><div className="dialog-title"><h2>{enabled?"确认打开所有检测？":"确认暂停所有检测？"}</h2><button type="button"className="icon-button"disabled={busy}onClick={onCancel}aria-label="关闭">×</button></div>
    <p>{enabled?"重新打开网络检测总开关，按各配置原来的开关选择恢复采样。":"暂停所有 YAML 配置的节点检测及本机 20 个网站检测，并取消正在执行的批次和手动 IP 评分。"}</p>
    <p className="hint">仅影响本页平台层网络检测。配置、单项开关和历史全部保留；恢复后不补造暂停期间的数据，IP 评分仍需手动发起。</p>
    {error&&<p className="form-error"role="alert">{error}</p>}<div className="dialog-actions"><button type="button"className="button button--secondary"disabled={busy}onClick={onCancel}>取消</button><button type="submit"className="button"disabled={busy}>{busy?"正在保存…":enabled?"确认打开所有检测":"确认暂停所有检测"}</button></div></form></dialog>;
}
export default function ProxyMonitoring({status,profile,local=false,onChanged}:{status?:ProxyLabStatus;profile?:ProxyProfile;local?:boolean;onChanged:(value:MonitoringChange)=>void}){
  const [busy,setBusy]=useState(false);const [confirm,setConfirm]=useState<boolean|null>(null);const [error,setError]=useState("");const [message,setMessage]=useState("");const settings=status?.monitoring;
  async function single(){
    if(busy||!settings||(!local&&!profile))return;setBusy(true);setError("");setMessage("");
    try{if(local){const result=await setLocalNetworkMonitoring(settings,!settings.local_enabled);onChanged({monitoring:result.monitoring});setMessage(result.message);}else if(profile){const result=await enableProxyProfile(profile,!profile.enabled);onChanged({profile:{id:profile.id,enabled:!profile.enabled}});setMessage(result.message);}}
    catch(e){setError(e instanceof Error?e.message:"检测开关保存失败");}finally{setBusy(false);}
  }
  async function all(){
    if(busy||!settings||confirm==null)return;setBusy(true);setError("");setMessage("");
    try{const result=await setProxyMonitoring(settings,confirm);onChanged({monitoring:result.monitoring});setMessage(result.message);setConfirm(null);}catch(e){setError(e instanceof Error?e.message:"总开关保存失败");}finally{setBusy(false);}
  }
  return <div className="proxy-monitoring">
    {settings&&(profile||local)&&<MonitoringSwitch label={local?"本机网站检测":"本配置检测"}enabled={local?settings.local_enabled:Boolean(profile?.enabled)}disabled={busy}onClick={()=>void single()}/>}
    <MonitoringSwitch label="所有检测"enabled={settings?.enabled??false}disabled={busy||!settings}onClick={()=>{if(settings){setError("");setConfirm(!settings.enabled);}}}/>
    {settings&&!settings.enabled&&<span className="hint proxy-monitoring__paused">总开关已关闭 · 单项选择保留</span>}
    {error&&confirm==null&&<span className="form-error"role="alert">{error}</span>}{message&&<span className="hint"role="status">{message}</span>}
    {confirm!=null&&<MonitoringConfirmation enabled={confirm}busy={busy}error={error}onConfirm={()=>void all()}onCancel={()=>setConfirm(null)}/>}
  </div>;
}
