import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { addMachine, changeMachine, fetchMachineJob, fetchMachines, probeMachine, manageMachineSsh } from "../api/client";
import type { ManagedMachine, MachineDeployment, MachineStatus } from "../api/client";
import { formatDate } from "../lib/format";
import { MAX_PRIVATE_KEY_BYTES, clearMachineAuthentication, type MachineAuthentication, type MachineAuthMethod, type MachineDeploymentMode } from "../lib/machineAuthentication";

export function MachineManagement({ onRefresh }: { onRefresh: () => void }) {
  const [status,setStatus]=useState<MachineStatus|null>(null);
  const [error,setError]=useState("");
  const [message,setMessage]=useState("");
  const [adding,setAdding]=useState(false);
  const [sshMachine,setSshMachine]=useState<ManagedMachine|null>(null);
  const [selected,setSelected]=useState<{machine:ManagedMachine;action:"edit"|"delete"|"restore"}|null>(null);
  const [job,setJob]=useState<{id:string;key:string}|null>(null);
  const [progress,setProgress]=useState<MachineDeployment|null>(null);
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
    <header className="panel-header"><div><h2>机器管理</h2><p>新增可选普通用户 / 管理员 · 默认 Basic，可后续提权 · 软删除保留历史和身份 · 当前仅支持 Linux</p></div>
      <button className="button button--small" disabled={!status?.candidate_available||!status.ca_available||!status.registry_writable||!!job} onClick={()=>{setError("");setAdding(true);}}>添加机器</button></header>
    {status&&!status.candidate_available&&<p className="hint">最新 Agent 候选尚未发布，请先构建并发布后添加。</p>}
    {status&&!status.ca_available&&<p className="hint">Agent 信任 CA 尚不可用，请配置 EASYMANAGE_AGENT_CA_CERTIFICATE。</p>}
    {status&&!status.registry_writable&&<p className="hint">可信清单目录不可由平台服务用户安全写入，请管理员配置；不会自动提权。</p>}
    {error&&<p className="form-error" role="alert">{error}</p>}{message&&<p className="notice notice--success" role="status">{message}</p>}
    {progress&&<section className="machine-progress" aria-label="Agent 部署进度">
      <h3>{progress.status==="success"?"部署完成":progress.status==="failed"?"部署未完成":"正在部署 Agent"} · {progress.progress}%</h3>
      <progress max={100} value={progress.progress}/><p className="hint">流程日志不含密码、注册密钥或原始 SSH 输出。关闭/刷新页面不会取消服务端任务；刷新会丢失本页的临时日志访问凭据，请以机器心跳核验。</p>
      <div className="deployment-log" role="log" aria-live="polite">{progress.events.map((event,index)=><div key={index}><time>{formatDate(event.at)}</time> {event.message}</div>)}</div>
    </section>}
    <details><summary>查看受管机器与已删除机器（{status?.machines.length??0}）</summary>
      <div className="table-scroll"><table><thead><tr><th>机器</th><th>SSH 地址 / 用户</th><th>状态</th><th>操作</th></tr></thead><tbody>
        {status?.machines.map(machine=><tr key={machine.node_id}><td><strong>{machine.display_name||machine.hostname}</strong><small className="block">{machine.note||machine.hostname}</small></td>
          <td>{machine.host?`${machine.host}:${machine.port}`:"未配置可信目标"}<small className="block">{machine.username}</small></td>
          <td>{machine.deleted_at?"已软删除":Date.now()-Date.parse(machine.last_seen)<45_000?"在线":"离线 / 等待部署"}
            {machine.connection_mode==="ssh"&&<small className="block">SSH · {sshLabel(machine.ssh_transport?.status)} · {machine.ssh_transport?.connections??0}/3 条连接</small>}</td>
          <td><div className="machine-actions"><button className="button button--small button--secondary" disabled={!machine.manageable||!!job} onClick={()=>setSelected({machine,action:"edit"})}>编辑</button>
            <button className="button button--small button--secondary" disabled={!machine.manageable||!!job} onClick={()=>setSelected({machine,action:machine.deleted_at?"restore":"delete"})}>{machine.deleted_at?"恢复":"软删除"}</button>
            {!machine.deleted_at&&machine.host&&<button className="button button--small button--secondary" disabled={!!job} onClick={()=>setSshMachine(machine)}>SSH 长连接</button>}</div></td></tr>)}
      </tbody></table></div>
    </details>
    {adding&&<AddMachineDialog onClose={()=>setAdding(false)} onStarted={started=>{setAdding(false);setProgress({node_id:started.node_id,status:"running",progress:0,events:[]});setJob({id:started.job_id,key:started.job_key});}}/>}
    {selected&&<ChangeMachineDialog {...selected} onClose={()=>setSelected(null)} onDone={message=>{setMessage(message);setSelected(null);void refresh();onRefresh();}}/>}
    {sshMachine&&<MachineSshDialog machine={sshMachine} onClose={()=>setSshMachine(null)} onDone={message=>{setMessage(message);setSshMachine(null);void refresh();onRefresh();}}/>}
  </article>;
}

function AddMachineDialog({onClose,onStarted}:{onClose:()=>void;onStarted:(job:{job_id:string;job_key:string;node_id:string})=>void}){
  const dialog=useRef<HTMLDialogElement>(null);
  const [name,setName]=useState("");const [host,setHost]=useState("");const [port,setPort]=useState(22);
  const [username,setUsername]=useState("");const [authMethod,setAuthMethod]=useState<MachineAuthMethod>("password");
  const [deploymentMode,setDeploymentMode]=useState<MachineDeploymentMode>("basic");
  const [connectionMode,setConnectionMode]=useState<"auto"|"direct"|"ssh">("auto");
  const sudoInput=useRef<HTMLInputElement>(null);
  const passwordInput=useRef<HTMLInputElement>(null);const keyInput=useRef<HTMLInputElement>(null);const passphraseInput=useRef<HTMLInputElement>(null);
  const [url,setUrl]=useState(window.location.origin);const [fingerprint,setFingerprint]=useState("");
  const [confirmed,setConfirmed]=useState(false);const [busy,setBusy]=useState(false);const [error,setError]=useState("");
  function clearInputs(){for(const input of [passwordInput.current,keyInput.current,passphraseInput.current,sudoInput.current])if(input)input.value="";}
  useEffect(()=>{dialog.current?.showModal();return clearInputs;},[]);
  async function submit(){
    if(busy)return;setError("");setBusy(true);
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
    <p className="hint">确认指纹前不发送鉴权凭据。SSH / sudo 密码、私钥和口令不保存到数据库、配置或日志。SSH 长连接期间为断线重连保留 SSH 凭据于平台内存，断开后清理；平台重启后须重新鉴权。请确保平台证书可信。</p>
    {error&&<p className="form-error" role="alert">{error}</p>}
    <div className="dialog-actions"><button type="button" className="button button--secondary" disabled={busy} onClick={onClose}>取消</button><button className="button" disabled={busy||!!fingerprint&&!confirmed}>{busy?"处理中…":fingerprint?"确认下发并启动":"检查 SSH 主机指纹"}</button></div>
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
      <p className="hint">选择私钥文件，不是 .pub 公钥；支持带口令私钥，私钥内容不会展示或写入服务器文件。</p>
    </>}
  </>;
}

function sshLabel(status?:string){
  return ({connected:"已连接",reconnecting:"断线重连中",authentication_required:"需重新鉴权",disconnected:"已断开"} as Record<string,string>)[status??""]??"等待连接";
}

function MachineSshDialog({machine,onClose,onDone}:{machine:ManagedMachine;onClose:()=>void;onDone:(message:string)=>void}){
  const dialog=useRef<HTMLDialogElement>(null);
  const [action,setAction]=useState<"connect"|"disconnect">(machine.ssh_transport&&machine.ssh_transport.status!=="disconnected"?"disconnect":"connect");
  const [method,setMethod]=useState<MachineAuthMethod>("password");
  const [busy,setBusy]=useState(false);const [error,setError]=useState("");const [confirmed,setConfirmed]=useState(false);
  const password=useRef<HTMLInputElement>(null);const key=useRef<HTMLInputElement>(null);const phrase=useRef<HTMLInputElement>(null);const sudo=useRef<HTMLInputElement>(null);
  function clear(){for(const input of [password.current,key.current,phrase.current,sudo.current])if(input)input.value="";}
  useEffect(()=>{dialog.current?.showModal();return clear;},[]);
  async function submit(){
    if(busy||!confirmed)return;setBusy(true);setError("");let auth:MachineAuthentication|undefined;
    try{
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
    }catch(error){setError(error instanceof Error?error.message:"SSH 网络切换失败");}
    finally{if(auth)clearMachineAuthentication(auth);clear();setBusy(false);}
  }
  return <dialog ref={dialog} className="credentials-dialog" onCancel={event=>{if(busy)event.preventDefault();else onClose();}}><form autoComplete="off" onSubmit={event=>{event.preventDefault();void submit();}}>
    <div className="dialog-title"><h2>SSH 长连接 · {machine.display_name||machine.hostname}</h2><button type="button" className="icon-button" disabled={busy} onClick={onClose} aria-label="关闭">×</button></div>
    <p>{machine.username}@{machine.host}:{machine.port} · {sshLabel(machine.ssh_transport?.status)} · {machine.ssh_transport?.connections??0}/3 条连接</p>
    <label>操作<select value={action} disabled={busy} onChange={event=>{setAction(event.target.value as typeof action);clear();setConfirmed(false);}}><option value="connect">建立 SSH 长连接并切换上报（会重启 Agent）</option><option value="disconnect">断开 SSH 长连接并释放内存凭据</option></select></label>
    <p className="hint">复用已核验主机密钥及原机器 ID。切换不会重装、降权或删除 SQLite；已断开/平台重启后须重新提交 SSH 鉴权。连接存在时请先断开再重连。</p>
    <SshAuthenticationFields method={method} onMethodChange={value=>{clear();setMethod(value);}} busy={busy} required passwordRef={password} keyRef={key} passphraseRef={phrase}/>
    {action==="connect"&&machine.username!=="root"&&<label>已有 Full 服务的 sudo 密码（如需）<input ref={sudo} type="password" autoComplete="off" disabled={busy} maxLength={1024}/><small>Basic 留空；已有 Full 使用密码鉴权时留空复用 SSH 密码，密钥鉴权时可单独填写。</small></label>}
    <p className="notice">SSH 凭据仅驻留平台运行内存，用于断线退避重连，不落盘。监控通常占用 2 条、最多 3 条 SSH 连接；需目标允许回环端口转发。平台重启会中断隧道，不会自动恢复凭据。</p>
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
