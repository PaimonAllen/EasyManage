import {useEffect,useRef,useState} from "react";
import {stopMachineAgent,type ManagedMachine,type AdminAuthentication} from "../api/client";
import {AdminCredentialFields,useAdminCredentials} from "./AdminCredentials";
import {SshAuthenticationFields} from "./MachineManagement";
import {MAX_PRIVATE_KEY_BYTES,type MachineAuthMethod} from "../lib/machineAuthentication";

export function agentServiceBlocked(machine:{agent_service_state?:string|null;agent_service_desired_stopped?:boolean}):boolean {
  return Boolean(machine.agent_service_desired_stopped || machine.agent_service_state&&machine.agent_service_state!=="running");
}

export function AgentStopButton({machine,busy,onStop}:{machine:ManagedMachine;busy:boolean;onStop:()=>void}){
  const pending=["stopping","redeploying"].includes(machine.agent_service_state??"");
  return <button type="button" className="button button--small button--danger" disabled={busy||pending||!machine.agent_stop_available||machine.agent_service_state==="stopped"}
    aria-label={`${machine.display_name||machine.hostname} 停止 Agent`}
    title="关闭 Agent 进程、心跳及自动拉起，保留安装和历史；继续使用只能重新部署" onClick={onStop}>{machine.agent_service_state==="stopped"?"Agent 已停止":pending?"处理中…":"停止 Agent"}</button>;
}

export function AgentStopDialog({machine,onClose,onStopped}:{machine:ManagedMachine;onClose:()=>void;onStopped:(message:string)=>void}){
  const full=machine.deployment_mode==="full";
  const auth=useAdminCredentials(full?[machine.node_id]:[]);
  const dialog=useRef<HTMLDialogElement>(null);const active=useRef(true);const pending=useRef(false);
  const [busy,setBusy]=useState(false);const [confirmed,setConfirmed]=useState(false);const [error,setError]=useState("");
  const [saved,setSaved]=useState(Boolean(machine.saved_credentials_usable));const [method,setMethod]=useState<MachineAuthMethod>("password");
  const password=useRef<HTMLInputElement>(null);const key=useRef<HTMLInputElement>(null);const passphrase=useRef<HTMLInputElement>(null);
  function clear(){for(const element of [password.current,key.current,passphrase.current])if(element)element.value="";}
  useEffect(()=>{active.current=true;const element=dialog.current;element?.showModal();return()=>{active.current=false;clear();element?.close();};},[]);
  async function submit(){
    if(pending.current||!confirmed)return;pending.current=true;setBusy(true);setError("");
    let credentials:AdminAuthentication|undefined;
    try{
      if(full)credentials=await auth.prepare();
      else if(saved)credentials={use_saved:true};
      else{
        credentials={username:machine.username||""};
        if(method==="password")credentials.password=password.current?.value||"";
        else{
          const file=key.current?.files?.[0];
          if(!file||!file.size||file.size>MAX_PRIVATE_KEY_BYTES||/\.(pub|ppk)$/i.test(file.name))throw new Error("请选择不超过 12 KiB 的 OpenSSH / PEM 私钥");
          credentials.private_key=await file.text();credentials.passphrase=passphrase.current?.value||"";
        }
      }
      const request=stopMachineAgent(machine,credentials);clear();
      if(!("use_saved" in credentials)){credentials.password="";credentials.private_key="";credentials.passphrase="";credentials.sudo_password="";}
      const result=await request;if(active.current)onStopped(result.message);
    }catch(error){if(active.current)setError(error instanceof Error?error.message:"停止 Agent 未完成，请核验实际状态");}
    finally{if(credentials&&!('use_saved' in credentials)){credentials.password="";credentials.private_key="";credentials.passphrase="";credentials.sudo_password="";}clear();pending.current=false;if(active.current)setBusy(false);}
  }
  return <dialog className="credentials-dialog" ref={dialog} onCancel={event=>{if(busy)event.preventDefault();else onClose();}}><form autoComplete="off" onSubmit={event=>{event.preventDefault();void submit();}}>
    <div className="dialog-title"><h2>停止 Agent · {machine.display_name||machine.hostname}</h2><button type="button" className="icon-button" disabled={busy} onClick={onClose} aria-label="关闭">×</button></div>
    <p className="notice">停止后关闭 Agent 进程、心跳和自动拉起，并断开此节点的 SSH 监控长连接。保留 Agent 文件、配置、原机器 ID、本地数据库和平台历史。继续使用只能重新部署 Agent。</p>
    <p className="hint">“暂停监控”保留轻量心跳，可直接恢复；本操作会停止 Agent 服务。平台与其他机器继续运行。</p>
    {full?<AdminCredentialFields auth={auth} disabled={busy}/>:<>
      <p>原 SSH 账户：{machine.username}@{machine.host}:{machine.port} · Basic，不调用 sudo</p>
      {machine.saved_credentials_usable&&<label className="machine-confirm"><input type="checkbox" checked={saved} disabled={busy} onChange={event=>{clear();setSaved(event.target.checked);}}/>使用平台已加密保存的 SSH 凭据</label>}
      {!saved&&<SshAuthenticationFields method={method} onMethodChange={value=>{clear();setMethod(value);}} busy={busy} required passwordRef={password} keyRef={key} passphraseRef={passphrase}/>}
    </>}
    <label className="machine-confirm"><input type="checkbox" checked={confirmed} disabled={busy} onChange={event=>setConfirmed(event.target.checked)}/>确认完全停止此 Agent；之后只能重新部署，不能用“恢复监控”拉起。</label>
    {error&&<p className="form-error" role="alert">{error}</p>}
    <div className="dialog-actions"><button type="button" className="button button--secondary" disabled={busy} onClick={onClose}>取消</button><button type="submit" className="button button--danger" disabled={busy||!confirmed||full&&!auth.ready}>{busy?"正在停止…":"确认停止 Agent"}</button></div>
  </form></dialog>;
}
