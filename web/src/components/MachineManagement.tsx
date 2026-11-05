import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { addMachine, changeMachine, fetchMachineJob, fetchMachines, probeMachine, manageMachineSsh, redeployMachine, setMachineMonitoring } from "../api/client";
import type { ManagedMachine, MachineDeployment, MachineStatus } from "../api/client";
import { formatDate } from "../lib/format";
import { MAX_PRIVATE_KEY_BYTES, clearMachineAuthentication, type MachineAuthentication, type MachineAuthMethod, type MachineDeploymentMode } from "../lib/machineAuthentication";
import { ExternalPlatformDialog, ExternalPlatformManagement } from "./ExternalPlatform";
import type { ExternalPlatform } from "../api/externalPlatforms";
import {AgentStopDialog,AgentStopButton,agentServiceBlocked} from "./AgentStopDialog";
import {AdminCredentialFields,useAdminCredentials} from "./AdminCredentials";

export function MachineManagement({ onRefresh, externalPlatforms=[], externalError="", onExternalChanged=()=>{} }: {
  onRefresh:()=>void;externalPlatforms?:ExternalPlatform[];externalError?:string;onExternalChanged?:()=>void;
}) {
  const [status,setStatus]=useState<MachineStatus|null>(null);
  const [error,setError]=useState("");
  const [message,setMessage]=useState("");
  const [adding,setAdding]=useState(false);
  const [sshMachine,setSshMachine]=useState<ManagedMachine|null>(null);
  const [redeployTarget,setRedeployTarget]=useState<ManagedMachine|null>(null);
  const [stopTarget,setStopTarget]=useState<ManagedMachine|null>(null);
  const [selected,setSelected]=useState<{machine:ManagedMachine;action:"edit"|"delete"|"restore"}|null>(null);
  const [job,setJob]=useState<{id:string;key:string}|null>(null);
  const [progress,setProgress]=useState<MachineDeployment|null>(null);
  const [monitoringBusy,setMonitoringBusy]=useState(false);
  async function toggleMonitoring(machine:ManagedMachine) {
    if(monitoringBusy || job)return;
    const paused=!machine.monitoring_paused;
    if(!window.confirm(`${paused?"暂停":"恢复"}「${machine.display_name||machine.hostname}」的 Agent 监控？\n暂停只停止指标采集和上报，保留轻量心跳与 SSH 长连接，恢复无需重装。${paused&&machine.monitoring_supported===false?"\n当前 Agent 尚未支持暂停：将保存请求，更新 Agent 后才能生效。":""}`))return;
    setMonitoringBusy(true);setError("");setMessage("");
    try {setMessage((await setMachineMonitoring(machine,paused)).message);await refresh();onRefresh();}
    catch(error){setError(error instanceof Error?error.message:"监控设置保存失败");}
    finally{setMonitoringBusy(false);}
  }
  const refresh=useCallback(async()=>{
    try {setStatus(await fetchMachines(AbortSignal.timeout(10_000)));setError("");}
    catch(error){setError(error instanceof Error?error.message:"机器管理接口不可用");}
  },[]);
  useEffect(()=>{void refresh();const timer=window.setInterval(()=>{if(!document.hidden)void refresh();},15_000);return()=>window.clearInterval(timer);},[refresh]);
  useEffect(()=>{
    if(!job)return;
    const controller=new AbortController();let timer:number|undefined;
    async function poll(){
      try {
        const current=await fetchMachineJob(job!.id,job!.key,controller.signal);
        if(controller.signal.aborted)return;
        setProgress(current);
        if(current.status==="running")timer=window.setTimeout(()=>void poll(),1_000);
        else{setJob(null);void refresh();onRefresh();}
      }catch(error){
        if(!controller.signal.aborted){const message=error instanceof Error?error.message:"部署进度读取失败";setError(message);if(message.includes("任务不存在或已过期")){setJob(null);void refresh();onRefresh();}else timer=window.setTimeout(()=>void poll(),3_000);}
      }
    }
    void poll();return()=>{controller.abort();window.clearTimeout(timer);};
  },[job,refresh,onRefresh]);
  return <article className="data-panel machine-management">
    <header className="panel-header"><div><h2>机器管理</h2><p>新增可选 Agent 机器或外部算力平台入口 · Agent 默认 Basic，可后续提权 · 软删除保留历史和身份 · Agent 当前仅支持 Linux</p><p>暂停监控：保留轻量心跳，可直接恢复。停止 Agent：关闭进程和心跳，保留文件与历史，只能重新部署后继续。</p></div>
      <button className="button button--small" disabled={!!job} onClick={()=>{setError("");setAdding(true);}}>添加机器</button></header>
    {status&&!status.candidate_available&&<p className="hint">最新 Agent 候选尚未发布，请先构建并发布后添加。</p>}
    {status&&!status.ca_available&&<p className="hint">Agent 信任 CA 尚不可用，请配置 EASYMANAGE_AGENT_CA_CERTIFICATE。</p>}
    {status&&!status.registry_writable&&<p className="hint">可信清单目录不可由平台服务用户安全写入，请管理员配置；不会自动提权。</p>}
    {error&&<p className="form-error" role="alert">{error}</p>}{message&&<p className="notice notice--success" role="status">{message}</p>}
    {progress&&<section className="machine-progress" aria-label="Agent 部署进度">
      <h3>{progress.status==="success"?"部署完成":progress.status==="failed"?"部署未完成":"正在部署 Agent"} · {progress.progress}%</h3>
      <progress max={100} value={progress.progress}/><p className="hint">流程日志不含密码、注册密钥或原始 SSH 输出。关闭/刷新页面不会取消服务端任务；刷新会丢失本页的临时日志访问凭据，请以机器心跳核验。</p>
      <div className="deployment-log" role="log" aria-live="polite">{progress.events.map((event,index)=><div key={index}><time>{formatDate(event.at)}</time> {event.message}</div>)}</div>
    </section>}
    <details className="machine-list" open><summary>查看受管机器与已删除机器（{status?.machines.length??0}）</summary>
      <div className="table-scroll"><table><thead><tr><th>机器</th><th>SSH 地址 / 用户</th><th>状态</th><th>操作</th></tr></thead><tbody>
        {status?.machines.map(machine=><tr key={machine.node_id}><td><strong>{machine.display_name||machine.hostname}</strong><small className="block">{machine.note||machine.hostname}</small></td>
          <td>{machine.host?`${machine.host}:${machine.port}`:"未配置可信目标"}<small className="block">{machine.username}</small></td>
          <td>{machine.deleted_at?"已软删除":machineMonitoringLabel(machine)}
            {!machine.deleted_at&&!agentServiceBlocked(machine)&&machine.monitoring_supported===false&&<small className="machine-monitoring-warning">暂停采集需先<a href="#/agent-management">更新 Agent ↗</a>；停止 Agent 无需升级</small>}
            {machine.agent_service_last_error&&<small className="form-error">{machine.agent_service_last_error}</small>}
            {machine.connection_mode==="ssh"&&<small className="block">SSH · {sshLabel(machine.ssh_transport?.status)} · {machine.ssh_transport?.connections??0}/3 条连接{machine.ssh_transport?.status==="reconnecting"?` · 重试 ${machine.ssh_transport.retry_attempt??0}/3（每 60 秒）`:""}</small>}
            {machine.ssh_transport?.last_error&&<small className="block">{machine.ssh_transport.last_error}</small>}
            {machine.credentials_saved&&<small className="block">SSH 凭据已加密保存</small>}</td>
          <td><div className="machine-actions"><button className="button button--small button--secondary" disabled={!machine.manageable||!!job} onClick={()=>setSelected({machine,action:"edit"})}>编辑</button>
            {!machine.deleted_at&&<MachineMonitoringButton machine={machine} busy={monitoringBusy||!!job} onToggle={()=>void toggleMonitoring(machine)}/>}
            {!machine.deleted_at&&<AgentStopButton machine={machine} busy={!!job} onStop={()=>setStopTarget(machine)}/>}
            <button className="button button--small button--secondary" disabled={!machine.manageable||!!job} onClick={()=>setSelected({machine,action:machine.deleted_at?"restore":"delete"})}>{machine.deleted_at?"恢复":"软删除"}</button>
            {!machine.deleted_at&&machine.host&&<><button className="button button--small button--secondary" disabled={!!job||agentServiceBlocked(machine)} onClick={()=>setSshMachine(machine)}>SSH 长连接</button>
              <button className="button button--small button--secondary" disabled={!!job||!machine.redeploy_available||!status?.candidate_available||!status.ca_available||!status.registry_writable}
                title="离线节点可覆盖修复原 Agent；保留原机器 ID、本地数据库和平台历史" onClick={()=>setRedeployTarget(machine)}>重新部署</button></>}</div></td></tr>)}
      </tbody></table></div>
    </details>
    {externalError&&<p className="form-error" role="alert">外部平台入口：{externalError}</p>}
    <ExternalPlatformManagement platforms={externalPlatforms} onChanged={onExternalChanged}/>
    {adding&&<AddMachineDialog agentAvailable={!!status?.candidate_available&&status.ca_available&&status.registry_writable} onClose={()=>setAdding(false)} onExternalSaved={()=>{setAdding(false);setMessage("外部算力平台入口已添加，可在下方卡片点击大按钮访问。");onExternalChanged();}} onStarted={started=>{setAdding(false);setProgress({node_id:started.node_id,status:"running",progress:0,events:[]});setJob({id:started.job_id,key:started.job_key});}}/>}
    {selected&&<ChangeMachineDialog {...selected} onClose={()=>setSelected(null)} onDone={message=>{setMessage(message);setSelected(null);void refresh();onRefresh();}}/>}
    {sshMachine&&<MachineSshDialog machine={sshMachine} onClose={()=>setSshMachine(null)} onDone={message=>{setMessage(message);setSshMachine(null);void refresh();onRefresh();}}/>}
    {redeployTarget&&<RedeployMachineDialog machine={redeployTarget} onClose={()=>setRedeployTarget(null)} onStarted={started=>{
      setRedeployTarget(null);setProgress({node_id:started.node_id,status:"running",progress:0,events:[]});setJob({id:started.job_id,key:started.job_key});
    }}/>}
    {stopTarget&&<AgentStopDialog machine={stopTarget} onClose={()=>setStopTarget(null)} onStopped={message=>{setStopTarget(null);setMessage(message);void refresh();onRefresh();}}/>}
  </article>;
}

export function machineMonitoringLabel(machine:ManagedMachine):string {
  if(machine.agent_service_state==="stopped")return "Agent 已停止 · 需重新部署";
  if(machine.agent_service_state==="stopping")return "正在停止 Agent";
  if(machine.agent_service_state==="redeploying")return "Agent 重新部署中";
  if(machine.agent_service_state==="unknown")return "Agent 服务状态未确认 · 请核验或重新部署";
  if(machine.monitoring_paused) {
    if(!machine.monitoring_supported)return "暂停待确认 · 需更新 Agent";
    if(machine.monitoring_applied_revision!==machine.monitoring_revision)return "暂停待确认";
    return machine.control_seen_at && Date.now()-Date.parse(machine.control_seen_at)<150_000 ? "已暂停 · 轻量心跳在线" : "已暂停 · 轻量心跳离线";
  }
  // Legacy Agents cannot acknowledge control revisions. Their real heartbeat
  // determines availability after resume, as on the backend's legacy path.
  if(machine.monitoring_supported!==false){
    if((machine.monitoring_revision??0)>0 && machine.monitoring_applied_revision!==machine.monitoring_revision)return "恢复待确认（最多约 1 分钟）";
    if((machine.monitoring_revision??0)>0 && machine.monitoring_reported_revision!==machine.monitoring_revision)return "已恢复 · 等待新指标";
  }
  return Date.now()-Date.parse(machine.last_seen)<45_000?"在线":"离线 / 等待部署";
}

export function MachineMonitoringButton({machine,busy,onToggle}:{machine:ManagedMachine;busy:boolean;onToggle:()=>void}) {
  const label=machine.monitoring_paused?"恢复监控":"暂停监控";
  return <button type="button" className={`button button--small button--secondary machine-monitoring-button machine-monitoring-button--${machine.monitoring_paused?"resume":"pause"}`} disabled={busy||machine.monitoring_revision===undefined||agentServiceBlocked(machine)}
    aria-label={`${machine.display_name||machine.hostname} ${label}`}
    title={agentServiceBlocked(machine)?"Agent 已停止或状态未确认，只能重新部署后继续":machine.monitoring_revision===undefined?"需先更新平台后端":!machine.monitoring_paused&&machine.monitoring_supported===false?"当前 Agent 需先更新，暂停采集请求会保留，更新后生效":"暂停采集，保留轻量心跳；可直接恢复监控"}
    onClick={onToggle}>{label}</button>;
}

export function RedeployMachineDialog({machine,onClose,onStarted}:{machine:ManagedMachine;onClose:()=>void;onStarted:(job:{job_id:string;job_key:string;node_id:string})=>void}){
  const useSeparateAdmin=machine.deployment_mode==="full"&&agentServiceBlocked(machine);
  const admin=useAdminCredentials(useSeparateAdmin?[machine.node_id]:[]);
  const dialog=useRef<HTMLDialogElement>(null);
  const [useSaved,setUseSaved]=useState(!!machine.saved_credentials_usable);
  const [method,setMethod]=useState<MachineAuthMethod>("password");
  const [connectionMode,setConnectionMode]=useState<"auto"|"direct"|"ssh">(machine.connection_mode==="ssh"?"ssh":"auto");
  const [url,setUrl]=useState(typeof window==="undefined"?"https://127.0.0.1:8443":window.location.origin);
  const [confirmed,setConfirmed]=useState(false);const [busy,setBusy]=useState(false);const [error,setError]=useState("");
  const password=useRef<HTMLInputElement>(null);const key=useRef<HTMLInputElement>(null);const phrase=useRef<HTMLInputElement>(null);const sudo=useRef<HTMLInputElement>(null);
  function clear(){for(const input of [password.current,key.current,phrase.current,sudo.current])if(input)input.value="";}
  useEffect(()=>{dialog.current?.showModal();return clear;},[]);
  async function submit(){
    if(busy||!confirmed)return;setBusy(true);setError("");let auth:MachineAuthentication|undefined;
    try{
      const request={confirmation:machine.node_id,username:machine.username||"",server_url:url.trim(),connection_mode:connectionMode,...(useSeparateAdmin?{administrator:await admin.prepare()}:{})};
      let pending:ReturnType<typeof redeployMachine>;
      if(useSaved)pending=redeployMachine(machine.node_id,{...request,use_saved:true});
      else{
        if(method==="password")auth={auth_method:"password",password:password.current?.value||""};
        else{
          const file=key.current?.files?.[0];
          if(!file||!file.size||file.size>MAX_PRIVATE_KEY_BYTES||/\.(pub|ppk)$/i.test(file.name))throw new Error("请选择不超过 12 KiB 的 OpenSSH / PEM 私钥文件");
          auth={auth_method:"private_key",private_key:await file.text(),passphrase:phrase.current?.value||""};
        }
        if(machine.deployment_mode==="full"&&machine.username!=="root")auth.sudo_password=sudo.current?.value||"";
        pending=redeployMachine(machine.node_id,{...request,...auth});
        clearMachineAuthentication(auth);
      }
      clear();onStarted(await pending);
    }catch(error){setError(error instanceof Error?error.message:"重新部署未启动");}
    finally{if(auth)clearMachineAuthentication(auth);clear();setBusy(false);}
  }
  return <dialog ref={dialog} className="credentials-dialog" onCancel={event=>{if(busy)event.preventDefault();else onClose();}}><form autoComplete="off" onSubmit={event=>{event.preventDefault();void submit();}}>
    <div className="dialog-title"><h2>重新部署 Agent · {machine.display_name||machine.hostname}</h2><button type="button" className="icon-button" disabled={busy} onClick={onClose} aria-label="关闭">×</button></div>
    <p>{machine.username}@{machine.host}:{machine.port}</p>
    {agentServiceBlocked(machine)&&<p className="notice">此机器的 Agent 已停止或服务状态未确认。重新部署会恢复 Agent 和监控，保留原机器 ID、历史与额外采集开关；仅启动旧服务或点“恢复监控”不能替代重新部署。</p>}
    <div className="notice"><strong>保留原机器 ID、历史、名称和备注</strong><code className="block">{machine.node_id}</code>
      <p>保持原权限：{machine.deployment_mode==="full"?"管理员 Full（root 系统服务）":"普通用户 Basic（全部位于原 SSH 用户 ~/.easymanage 内）"}。此入口不更换地址、账户、主机公钥或权限级别。</p></div>
    <p className="hint">支持离线节点覆盖修复，也支持已卸载后重装。核验属于同一机器 ID 后，备份并覆盖程序、配置和服务，仅重启本 Agent；保留原 SQLite 身份、待上报数据、采集设置和平台历史，不删除数据库、不重启机器。其他机器 ID 或 Basic / Full 冲突仍会拒绝覆盖。连接凭据会重新签发，原机器 ID 不变。</p>
    <AgentConnectionSettings url={url} busy={busy} onChange={setUrl} mode={connectionMode} onModeChange={setConnectionMode}/>
    {useSeparateAdmin&&<><p className="hint">普通用户 SSH 用于原账户的文件传输；管理员授权用于恢复 Full 系统服务，可使用不同管理员账户。</p><AdminCredentialFields auth={admin} disabled={busy}/></>}
    {machine.saved_credentials_usable&&<label className="machine-confirm"><input type="checkbox" checked={useSaved} disabled={busy} onChange={event=>{clear();setUseSaved(event.target.checked);}}/>使用平台已加密保存的 SSH 凭据</label>}
    {!useSaved&&<SshAuthenticationFields method={method} onMethodChange={value=>{clear();setMethod(value);}} busy={busy} required passwordRef={password} keyRef={key} passphraseRef={phrase}/>}
    {!useSaved&&!useSeparateAdmin&&machine.deployment_mode==="full"&&machine.username!=="root"&&<label>sudo 密码（如需）<input ref={sudo} type="password" autoComplete="off" maxLength={1024} disabled={busy}/><small>密码登录时留空复用 SSH 密码；私钥登录可单独提供 sudo 密码。</small></label>}
    <label className="machine-confirm"><input type="checkbox" checked={confirmed} disabled={busy} onChange={event=>setConfirmed(event.target.checked)}/>确认按以上原机器 ID 覆盖修复并重启 Agent；保留历史和本地数据库，不新增机器。</label>
    <p className="hint">凭据仅以密文保存在平台，不回传明文、不写日志；复用可信 SSH，最多 3 条长连接。任务进度和日志会显示在机器管理中。</p>
    {error&&<p className="form-error" role="alert">{error}</p>}
    <div className="dialog-actions"><button type="button" className="button button--secondary" disabled={busy} onClick={onClose}>取消</button><button className="button" disabled={busy||!confirmed||useSeparateAdmin&&!admin.ready}>{busy?"提交中…":"确认重新部署"}</button></div>
  </form></dialog>;
}

type EntryKind="agent"|"external";
function MachineKindSelector({kind,onChange,busy,agentAvailable}:{kind:EntryKind;onChange:(kind:EntryKind)=>void;busy:boolean;agentAvailable:boolean}){
  return <><label>接入方式<select value={kind} disabled={busy} onChange={event=>onChange(event.target.value as EntryKind)}><option value="agent" disabled={!agentAvailable}>SSH 部署 Agent 机器</option><option value="external">外部算力平台（链接入口）</option></select></label>
    {!agentAvailable&&<p className="hint">Agent 部署条件尚未就绪；外部算力平台入口可直接添加。</p>}</>;
}
export function AddMachineDialog({onClose,onStarted,onExternalSaved,agentAvailable}:{onClose:()=>void;onStarted:(job:{job_id:string;job_key:string;node_id:string})=>void;onExternalSaved:()=>void;agentAvailable:boolean}){
  const [kind,setKind]=useState<EntryKind>(agentAvailable?"agent":"external");
  return kind==="external"?<ExternalPlatformDialog onClose={onClose} onSaved={onExternalSaved} selector={busy=><MachineKindSelector kind={kind} onChange={setKind} busy={busy} agentAvailable={agentAvailable}/>}/>
    :<AddAgentMachineDialog onClose={onClose} onStarted={onStarted} onKindChange={setKind} agentAvailable={agentAvailable}/>;
}
function AddAgentMachineDialog({onClose,onStarted,onKindChange,agentAvailable}:{onClose:()=>void;onStarted:(job:{job_id:string;job_key:string;node_id:string})=>void;onKindChange:(kind:EntryKind)=>void;agentAvailable:boolean}){
  const dialog=useRef<HTMLDialogElement>(null);
  const [name,setName]=useState("");const [host,setHost]=useState("");const [port,setPort]=useState(22);
  const [username,setUsername]=useState("");const [authMethod,setAuthMethod]=useState<MachineAuthMethod>("password");
  const [deploymentMode,setDeploymentMode]=useState<MachineDeploymentMode>("basic");
  const [connectionMode,setConnectionMode]=useState<"auto"|"direct"|"ssh">("auto");
  const sudoInput=useRef<HTMLInputElement>(null);
  const passwordInput=useRef<HTMLInputElement>(null);const keyInput=useRef<HTMLInputElement>(null);const passphraseInput=useRef<HTMLInputElement>(null);
  const [url,setUrl]=useState(()=>typeof window==="undefined"?"https://127.0.0.1:8443":window.location.origin);const [fingerprint,setFingerprint]=useState("");
  const [confirmed,setConfirmed]=useState(false);const [busy,setBusy]=useState(false);const [error,setError]=useState("");
  function clearInputs(){for(const input of [passwordInput.current,keyInput.current,passphraseInput.current,sudoInput.current])if(input)input.value="";}
  useEffect(()=>{dialog.current?.showModal();return clearInputs;},[]);
  async function submit(){
    if(busy||!agentAvailable)return;setError("");setBusy(true);
    try{
      if(!fingerprint){
        const result=await probeMachine(host.trim(),port);setFingerprint(result.fingerprint);setConfirmed(false);
      }else{
        if(!confirmed)throw new Error("请先确认 SSH 主机指纹");
        if(deploymentMode==="basic"&&username.trim()==="root")throw new Error("普通用户模式请使用非 root SSH 账户；root 请选管理员级别");
        let authentication:MachineAuthentication;
        if(authMethod==="password")authentication={auth_method:"password",password:passwordInput.current?.value||""};
        else{
          const file=keyInput.current?.files?.[0];
          if(!file||file.size===0||file.size>MAX_PRIVATE_KEY_BYTES)throw new Error("请选择不超过 12 KiB 的 OpenSSH / PEM 私钥文件");
          if(/\.(pub|ppk)$/i.test(file.name))throw new Error("请使用 OpenSSH / PEM 私钥，不是 .pub 公钥或 PuTTY .ppk 文件");
          authentication={auth_method:"private_key",private_key:await file.text(),passphrase:passphraseInput.current?.value||""};
        }
        let pending:ReturnType<typeof addMachine>;
        if(deploymentMode==="full"&&username.trim()!=="root")authentication.sudo_password=sudoInput.current?.value||"";
        try{pending=addMachine({host:host.trim(),port,display_name:name.trim(),username:username.trim(),server_url:url.trim(),fingerprint,deployment_mode:deploymentMode,connection_mode:connectionMode,...authentication});}
        finally{clearMachineAuthentication(authentication);clearInputs();}
        onStarted(await pending);
      }
    }catch(caught){if(fingerprint)clearInputs();setError(caught instanceof Error?caught.message:"新增失败");}
    finally{setBusy(false);}
  }
  return <dialog ref={dialog} className="credentials-dialog" onCancel={event=>{if(busy)event.preventDefault();else onClose();}}><form autoComplete="off" onSubmit={event=>{event.preventDefault();void submit();}}>
    <div className="dialog-title"><h2>添加机器并部署 Agent</h2><button type="button" className="icon-button" disabled={busy} onClick={onClose} aria-label="关闭">×</button></div>
    <MachineKindSelector kind="agent" busy={busy} agentAvailable={agentAvailable} onChange={kind=>{clearInputs();onKindChange(kind);}}/>
    <p className="hint">无需预装 Agent：填写新机器的 SSH 信息并选择运行权限，平台将直接下发最新 Agent、安装并启动，随后自动等待对应权限的心跳。部署进度和日志会显示在机器管理中。</p>
    <AgentDeploymentFields mode={deploymentMode} onChange={mode=>{clearInputs();setDeploymentMode(mode);setConfirmed(false);}} busy={busy} username={username} sudoRef={sudoInput}/>
    <details><summary>目标机器要求（仅 Linux）</summary><p className="hint">目标需 Python3 和匹配的 CPU 架构。普通用户自动选择 systemd 用户服务或无 linger 兼容守护，无需 sudo。兼容模式在允许用户 cron 时配置开机及每分钟恢复；否则不保证开机/退出会话后的自动恢复。管理员模式需 systemd 及 root/sudo 授权。不覆盖已有 Agent 身份。</p></details>
    <label>显示名称<input required maxLength={160} value={name} disabled={busy} onChange={event=>setName(event.target.value)}/></label>
    <label>目标内网 IP<input required value={host} disabled={busy} onChange={event=>{setHost(event.target.value);setFingerprint("");setConfirmed(false);}}/></label>
    <label>SSH 端口<input required type="number" min={1} max={65535} value={port} disabled={busy} onChange={event=>{setPort(Number(event.target.value));setFingerprint("");setConfirmed(false);}}/></label>
    <AgentConnectionSettings url={url} busy={busy} onChange={setUrl} mode={connectionMode} onModeChange={setConnectionMode}/>
    <label>SSH 用户名<input required={!!fingerprint} autoComplete="off" maxLength={64} value={username} disabled={busy} onChange={event=>setUsername(event.target.value)}/></label>
    <SshAuthenticationFields method={authMethod} onMethodChange={method=>{clearInputs();setAuthMethod(method);}} busy={busy} required={!!fingerprint}
      passwordRef={passwordInput} keyRef={keyInput} passphraseRef={passphraseInput}/>
    {fingerprint&&<><div className="notice"><strong>SSH 主机指纹</strong><code className="block">{fingerprint}</code><p>请在目标机核对对应主机公钥的 ssh-keygen -lf 输出；首次连接不等于可信。</p></div>
      <label className="machine-confirm"><input type="checkbox" checked={confirmed} disabled={busy} onChange={event=>setConfirmed(event.target.checked)}/>我已核验指纹，确认此目标可安装并启动{deploymentMode==="full"?"管理员 Full 系统服务":"普通用户 Basic Agent"}</label>
    </>}
    <p className="hint">确认指纹前不发送鉴权凭据。验证成功后，SSH 密码、私钥及口令使用平台专用主密钥加密保存，不回传浏览器、不写入日志；平台重启后可恢复已启用的 SSH 长连接。请确保平台证书可信。</p>
    {error&&<p className="form-error" role="alert">{error}</p>}
    <div className="dialog-actions"><button type="button" className="button button--secondary" disabled={busy} onClick={onClose}>取消</button><button className="button" disabled={!agentAvailable||busy||!!fingerprint&&!confirmed}>{busy?"处理中…":fingerprint?"确认下发并启动":"检查 SSH 主机指纹"}</button></div>
  </form></dialog>;
}

export function AgentDeploymentFields({mode,onChange,busy,username,sudoRef}:{mode:MachineDeploymentMode;onChange:(mode:MachineDeploymentMode)=>void;busy:boolean;username:string;sudoRef:RefObject<HTMLInputElement|null>}){
  return <>
    <label>Agent 运行权限<select value={mode} disabled={busy} onChange={event=>onChange(event.target.value as MachineDeploymentMode)}>
      <option value="basic">普通用户 Basic（默认，后续可提权）</option><option value="full">管理员 Full（首次直接以 root 运行）</option>
    </select></label>
    {mode==="basic"?<p className="hint">使用 SSH 登录的普通用户运行；安装包、程序、配置及状态库均在该用户 ~/.easymanage 内，不执行 sudo，也不强制要求 linger。优先 systemd 用户服务；不满足时自动使用兼容守护，日志位于 ~/.easymanage/logs。需要时可在设备总览中提权到 Full。</p>:<>
      <p className="notice">将安装 root 系统服务：程序 /usr/local/sbin/easymanage-agent，配置 /etc/easymanage，状态库 /var/lib/easymanage-agent。不修改已有 Agent，也不会重启目标机器。</p>
      {username.trim()!=="root"&&<label>sudo 密码（如需）<input ref={sudoRef} type="password" autoComplete="off" maxLength={1024} disabled={busy}/><small>密码登录时留空默认使用 SSH 密码；密钥登录时填写 sudo 密码，免密 sudo 可留空。</small></label>}
    </>}
  </>;
}

export function AgentConnectionSettings({url,busy,onChange,mode="auto",onModeChange}:{url:string;busy:boolean;onChange:(url:string)=>void;mode?:"auto"|"direct"|"ssh";onModeChange?:(mode:"auto"|"direct"|"ssh")=>void}){
  return <>
    <label>监控连接方式<select value={mode} disabled={busy} onChange={event=>onModeChange?.(event.target.value as "auto"|"direct"|"ssh")}>
      <option value="auto">自动：HTTPS 不可达时使用 SSH 长连接</option><option value="ssh">SSH 长连接（只需目标 SSH 端口）</option><option value="direct">仅 HTTPS 直连</option>
    </select></label>
    <p className="hint">平台连接地址默认使用当前页面地址，自动写入 Agent 配置。SSH 模式由平台主动连接目标，不要求目标回连平台；通常保持 2 条、最多 3 条 SSH，命令/文件复用通道，不为每次采集重新登录。</p>
    <details><summary>高级网络设置（通常无需修改）</summary>
      <label>Agent 上报地址<input required type="url" value={url} disabled={busy} onChange={event=>onChange(event.target.value)}/></label>
      <p className="hint">此地址用于 HTTPS 直连，不是目标机器地址。SSH 模式自动使用目标回环隧道，不需要填写目标能回连的平台地址；需管理员允许 SSH 回环端口转发。隧道继续校验 CA/证书。</p>
    </details>
  </>;
}

export function SshAuthenticationFields({method,onMethodChange,busy,required,passwordRef,keyRef,passphraseRef}:{
  method:MachineAuthMethod;onMethodChange:(method:MachineAuthMethod)=>void;busy:boolean;required:boolean;
  passwordRef:RefObject<HTMLInputElement|null>;keyRef:RefObject<HTMLInputElement|null>;passphraseRef:RefObject<HTMLInputElement|null>;
}){
  return <>
    <label>鉴权方式<select value={method} disabled={busy} onChange={event=>onMethodChange(event.target.value as MachineAuthMethod)}>
      <option value="password">密码登录</option><option value="private_key">SSH 私钥登录</option>
    </select></label>
    {method==="password"?<label>SSH 密码<input ref={passwordRef} required={required} type="password" autoComplete="off" maxLength={1024} disabled={busy}/></label>:<>
      <label>SSH 私钥文件（OpenSSH / PEM，最大 12 KiB）<input ref={keyRef} required={required} type="file" disabled={busy}/></label>
      <label>私钥口令（如有）<input ref={passphraseRef} type="password" autoComplete="off" maxLength={1024} disabled={busy}/></label>
      <p className="hint">选择私钥文件，不是 .pub 公钥；支持带口令私钥，私钥内容不会展示，平台仅保存加密副本。</p>
    </>}
  </>;
}

function sshLabel(status?:string){
  return ({connected:"已连接",reconnecting:"断线重连中",retry_exhausted:"3 次重连失败，已停止自动尝试",authentication_required:"凭据失效或主机密钥变化",disconnected:"已断开"} as Record<string,string>)[status??""]??"等待连接";
}

export function MachineSshDialog({machine,onClose,onDone}:{machine:ManagedMachine;onClose:()=>void;onDone:(message:string)=>void}){
  const dialog=useRef<HTMLDialogElement>(null);
  const [action,setAction]=useState<"connect"|"disconnect"|"forget">(machine.ssh_transport?.status==="connected"?"disconnect":"connect");
  const [savedUsable,setSavedUsable]=useState(!!machine.saved_credentials_usable);
  const [useSaved,setUseSaved]=useState(!!machine.saved_credentials_usable);
  const [method,setMethod]=useState<MachineAuthMethod>("password");
  const [busy,setBusy]=useState(false);const [error,setError]=useState("");const [confirmed,setConfirmed]=useState(false);
  const password=useRef<HTMLInputElement>(null);const key=useRef<HTMLInputElement>(null);const phrase=useRef<HTMLInputElement>(null);const sudo=useRef<HTMLInputElement>(null);
  function clear(){for(const input of [password.current,key.current,phrase.current,sudo.current])if(input)input.value="";}
  useEffect(()=>{dialog.current?.showModal();return clear;},[]);
  async function submit(){
    if(busy||!confirmed)return;setBusy(true);setError("");let auth:MachineAuthentication|undefined;
    try{
      if(useSaved){
        const pending=manageMachineSsh(machine.node_id,{action,username:machine.username||"",use_saved:true});
        clear();onDone((await pending).message);return;
      }
      if(method==="password")auth={auth_method:"password",password:password.current?.value||""};
      else{
        const file=key.current?.files?.[0];
        if(!file||!file.size||file.size>MAX_PRIVATE_KEY_BYTES||/\.(pub|ppk)$/i.test(file.name))throw new Error("请选择不超过 12 KiB 的 OpenSSH / PEM 私钥文件");
        auth={auth_method:"private_key",private_key:await file.text(),passphrase:phrase.current?.value||""};
      }
      if(action==="connect"&&sudo.current?.value)auth.sudo_password=sudo.current.value;
      const pending=manageMachineSsh(machine.node_id,{action,username:machine.username||"",...auth});
      clearMachineAuthentication(auth);clear();
      onDone((await pending).message);
    }catch(error){
      setError(error instanceof Error?error.message:"SSH 网络切换失败");
      // SSH authentication can succeed even when the later network preflight
      // fails. Its secure cookie lets the next attempt reuse encrypted secrets.
      try {const current=(await fetchMachines()).machines.find(item=>item.node_id===machine.node_id);
        setSavedUsable(!!current?.saved_credentials_usable);setUseSaved(!!current?.saved_credentials_usable);
      }catch{/* Preserve the original operation error. */}
    }
    finally{if(auth)clearMachineAuthentication(auth);clear();setBusy(false);}
  }
  return <dialog ref={dialog} className="credentials-dialog" onCancel={event=>{if(busy)event.preventDefault();else onClose();}}><form autoComplete="off" onSubmit={event=>{event.preventDefault();void submit();}}>
    <div className="dialog-title"><h2>SSH 长连接 · {machine.display_name||machine.hostname}</h2><button type="button" className="icon-button" disabled={busy} onClick={onClose} aria-label="关闭">×</button></div>
    <p>{machine.username}@{machine.host}:{machine.port} · {sshLabel(machine.ssh_transport?.status)} · {machine.ssh_transport?.connections??0}/3 条连接</p>
    <label>操作<select value={action} disabled={busy} onChange={event=>{setAction(event.target.value as typeof action);clear();setConfirmed(false);}}><option value="connect">建立 / 重连 SSH 长连接并切换上报（会重启 Agent）</option><option value="disconnect">断开并暂停自动恢复（保留加密凭据）</option><option value="forget">断开并删除平台保存的凭据</option></select></label>
    <p className="hint">复用已核验主机密钥及原机器 ID，不重装、不降权、不删除 SQLite。平台重启后自动恢复此前启用的连接；手动断开不会自动恢复。</p>
    {savedUsable&&<label className="machine-confirm"><input type="checkbox" checked={useSaved} disabled={busy} onChange={event=>{clear();setUseSaved(event.target.checked);}}/>使用平台已加密保存的 SSH 凭据（无需重复输入）</label>}
    {!useSaved&&<SshAuthenticationFields method={method} onMethodChange={value=>{clear();setMethod(value);}} busy={busy} required passwordRef={password} keyRef={key} passphraseRef={phrase}/>}
    {!useSaved&&action==="connect"&&machine.username!=="root"&&<label>已有 Full 服务的 sudo 密码（如需）<input ref={sudo} type="password" autoComplete="off" disabled={busy} maxLength={1024}/><small>Basic 留空；已有 Full 使用密码鉴权时留空复用 SSH 密码，密钥鉴权时可单独填写并加密保存。</small></label>}
    <p className="notice">凭据以 AES-256-GCM 加密保存在平台私有保险库，不返回明文。断线每 60 秒重试，最多 3 次；失败后停止，可手动重连。监控通常占用 2 条、最多 3 条 SSH 连接，需允许回环端口转发。新浏览器需验证一次 SSH。</p>
    <label className="machine-confirm"><input type="checkbox" checked={confirmed} disabled={busy} onChange={event=>setConfirmed(event.target.checked)}/>确认操作此机器；断开后经隧道上报会暂停，安装及身份仍保留。</label>
    {error&&<p className="form-error" role="alert">{error}</p>}<div className="dialog-actions"><button type="button" className="button button--secondary" disabled={busy} onClick={onClose}>取消</button><button className="button" disabled={busy||!confirmed}>{busy?"处理中…":"确认"}</button></div>
  </form></dialog>;
}

function ChangeMachineDialog({machine,action,onClose,onDone}:{machine:ManagedMachine;action:"edit"|"delete"|"restore";onClose:()=>void;onDone:(message:string)=>void}){
  const dialog=useRef<HTMLDialogElement>(null);const [name,setName]=useState(machine.display_name||machine.hostname);const [note,setNote]=useState(machine.note);
  const [username,setUsername]=useState(machine.username||"");const [password,setPassword]=useState("");const [scope,setScope]=useState<"target"|"platform_admin">("target");
  const [busy,setBusy]=useState(false);const [error,setError]=useState("");const [confirmed,setConfirmed]=useState(false);
  useEffect(()=>{dialog.current?.showModal();},[]);
  return <dialog ref={dialog} className="credentials-dialog" onCancel={event=>{if(busy)event.preventDefault();else onClose();}}><form onSubmit={event=>{
    event.preventDefault();if(busy)return;setBusy(true);setError("");
    const promise=changeMachine(machine.node_id,{action,display_name:name.trim(),note,username:username.trim(),password,confirmation:machine.node_id,auth_scope:scope});
    setPassword("");void promise.then(result=>onDone(result.message)).catch(error=>setError(error instanceof Error?error.message:"操作失败")).finally(()=>setBusy(false));
  }}>
    <div className="dialog-title"><h2>{action==="edit"?"编辑":action==="delete"?"软删除":"恢复"} {machine.display_name||machine.hostname}</h2><button type="button" className="icon-button" disabled={busy} onClick={onClose} aria-label="关闭">×</button></div>
    {action==="edit"?<><label>显示名称<input required value={name} maxLength={160} disabled={busy} onChange={event=>setName(event.target.value)}/></label><label>备注<textarea value={note} maxLength={2000} disabled={busy} onChange={event=>setNote(event.target.value)}/></label><p className="hint">编辑不改变 SSH 地址、机器身份或 Agent 权限；地址变更需要单独核验主机密钥。</p></>:<p className="hint">{action==="delete"?"仅从当前总览隐藏并停用上报凭据。历史指标、机器 ID、远端 Agent 服务和身份文件全部保留，不卸载服务。":"恢复原 ID 与上报凭据，不重新安装、不重建身份；后续状态以 Agent 心跳为准。"}</p>}
    <label>鉴权目标<select value={scope} disabled={busy} onChange={event=>{setScope(event.target.value as typeof scope);setUsername(event.target.value==="target"?machine.username||"":"");}}><option value="target">此机器 SSH（登记用户或管理员）</option><option value="platform_admin">平台本机管理员 SSH（目标离线时可用）</option></select></label>
    <label>SSH 用户名<input required autoComplete="off" value={username} disabled={busy} onChange={event=>setUsername(event.target.value)}/></label><label>密码<input required type="password" autoComplete="off" value={password} disabled={busy} onChange={event=>setPassword(event.target.value)}/></label>
    <label className="machine-confirm"><input type="checkbox" checked={confirmed} disabled={busy} onChange={event=>setConfirmed(event.target.checked)}/>确认操作机器 ID：<code>{machine.node_id}</code></label>
    {error&&<p className="form-error" role="alert">{error}</p>}<div className="dialog-actions"><button type="button" className="button button--secondary" disabled={busy} onClick={onClose}>取消</button><button className="button" disabled={busy||!confirmed}>{busy?"处理中…":"确认"}</button></div>
  </form></dialog>;
}
