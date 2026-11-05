import { useEffect, useRef, useState, type ReactNode } from "react";
import { changeExternalPlatform, saveExternalPlatform, type ExternalPlatform as Platform } from "../api/externalPlatforms";
import { externalPlatformUrl } from "../lib/externalPlatforms";
import "./ExternalPlatform.css";

export function ExternalPlatformDialog({ platform, onClose, onSaved, selector }: {
  platform?:Platform; onClose:()=>void; onSaved:(platform:Platform)=>void; selector?:(busy:boolean)=>ReactNode;
}) {
  const dialog=useRef<HTMLDialogElement>(null);
  const [name,setName]=useState(platform?.name??"");
  const [url,setUrl]=useState(platform?.url??"");
  const [note,setNote]=useState(platform?.note??"");
  const [busy,setBusy]=useState(false); const [error,setError]=useState("");
  const active=useRef(true);const pending=useRef(false);
  useEffect(()=>{active.current=true;const element=dialog.current;element?.showModal();return()=>{active.current=false;element?.close();};},[]);
  async function submit(){
    if(pending.current)return;
    const target=externalPlatformUrl(url);
    if(!name.trim()||!target){setError("请填写名称，以及完整的 http:// 或 https:// 平台链接；链接不能包含用户名和密码。");return;}
    pending.current=true;setBusy(true);setError("");
    try{const saved=await saveExternalPlatform({name:name.trim(),url:target,note:note.trim()},platform);if(active.current)onSaved(saved);}
    catch(error){if(active.current)setError(error instanceof Error?error.message:"入口保存失败");}
    finally{pending.current=false;if(active.current)setBusy(false);}
  }
  return <dialog ref={dialog} className="credentials-dialog" onCancel={event=>{if(busy)event.preventDefault();else onClose();}}>
    <form onSubmit={event=>{event.preventDefault();void submit();}}>
      <div className="dialog-title"><h2>{platform?"编辑外部算力平台":"添加外部算力平台"}</h2><button type="button" className="icon-button" disabled={busy} onClick={onClose} aria-label="关闭">×</button></div>
      {selector?.(busy)}
      <p className="hint">在设备总览创建访问入口，点击卡片中的大按钮进入外部算力平台；无需 SSH 信息或 Agent 部署。</p>
      <label>显示名称<input required autoFocus maxLength={128} value={name} disabled={busy} placeholder="例如：学部算力平台" onChange={event=>setName(event.target.value)}/></label>
      <label>平台链接<input required type="url" maxLength={4096} value={url} disabled={busy} placeholder="https://compute.example.com/" onChange={event=>setUrl(event.target.value)}/></label>
      <label>备注（可选）<textarea maxLength={1000} rows={3} value={note} disabled={busy} placeholder="用途、所属单位或使用说明" onChange={event=>setNote(event.target.value)}/></label>
      {error&&<p className="form-error" role="alert">{error}</p>}
      <div className="dialog-actions"><button type="button" className="button button--secondary" disabled={busy} onClick={onClose}>取消</button><button className="button" disabled={busy}>{busy?"保存中…":platform?"保存入口":"添加平台入口"}</button></div>
    </form>
  </dialog>;
}

export function ExternalPlatformCard({platform,onChanged}:{platform:Platform;onChanged:()=>void}){
  const [editing,setEditing]=useState(false);const [busy,setBusy]=useState(false);const [error,setError]=useState("");
  const url=externalPlatformUrl(platform.url);
  async function remove(){
    if(busy||!window.confirm(`软删除“${platform.name}”的入口？可在机器管理中恢复。`))return;
    setBusy(true);setError("");
    try{await changeExternalPlatform(platform,"delete");onChanged();}
    catch(error){setError(error instanceof Error?error.message:"软删除失败");}
    finally{setBusy(false);}
  }
  return <>
    <article className="node-card external-platform-card" id={`external-platform-${platform.id}`}>
      <header className="node-card__header"><div className="node-card__identity"><div className="node-card__title-row"><h2>{platform.name}</h2><span className="badge external-platform-badge">外部算力平台</span></div>
        <p className="external-platform-card__url" title={platform.url}>{url??"链接格式无效，请编辑修复"}</p></div>
        <div className="node-card__actions"><button type="button" className="button button--small button--secondary" disabled={busy} onClick={()=>setEditing(true)}>编辑</button><button type="button" className="button button--small button--secondary" disabled={busy} onClick={()=>void remove()}>软删除</button></div>
      </header>
      <div className="external-platform-card__body"><p>{platform.note||"通过下方入口进入该平台，查看和使用外部算力资源。"}</p>
        {url?<a className="button external-platform-card__launch" href={url} target="_blank" rel="noopener noreferrer" aria-label={`前往 ${platform.name}（新标签页打开）`}>前往算力平台 <span aria-hidden="true">↗</span></a>:<p className="form-error">入口链接无效，请编辑后访问。</p>}
        {error&&<p className="form-error" role="alert">{error}</p>}
      </div>
    </article>
    {editing&&<ExternalPlatformDialog platform={platform} onClose={()=>setEditing(false)} onSaved={()=>{setEditing(false);onChanged();}}/>}
  </>;
}

export function ExternalPlatformManagement({platforms,onChanged}:{platforms:Platform[];onChanged:()=>void}){
  const [editing,setEditing]=useState<Platform|null>(null);const [busy,setBusy]=useState(false);const [error,setError]=useState("");
  async function change(platform:Platform){
    const restore=!!platform.deleted_at;
    if(busy||!window.confirm(`${restore?"恢复":"软删除"}“${platform.name}”的平台入口？`))return;
    setBusy(true);setError("");
    try{await changeExternalPlatform(platform,restore?"restore":"delete");onChanged();}
    catch(error){setError(error instanceof Error?error.message:"操作失败");}
    finally{setBusy(false);}
  }
  return <>
    <details className="external-platform-management"><summary>外部平台入口管理（{platforms.length}）</summary>
      {error&&<p className="form-error" role="alert">{error}</p>}
      <div className="table-scroll"><table><thead><tr><th>名称 / 备注</th><th>平台链接</th><th>状态</th><th>操作</th></tr></thead><tbody>{platforms.map(platform=><tr key={platform.id}>
        <td><strong>{platform.name}</strong><small>{platform.note}</small></td><td><code>{platform.url}</code></td><td>{platform.deleted_at?"已软删除":"外部平台入口"}</td>
        <td><div className="machine-actions">{!platform.deleted_at&&<button type="button" className="button button--small button--secondary" disabled={busy} onClick={()=>setEditing(platform)}>编辑</button>}
          <button type="button" className="button button--small button--secondary" disabled={busy} onClick={()=>void change(platform)}>{platform.deleted_at?"恢复":"软删除"}</button></div></td>
      </tr>)}</tbody></table>{!platforms.length&&<p className="table-empty">尚未添加外部平台入口。</p>}</div>
    </details>
    {editing&&<ExternalPlatformDialog platform={editing} onClose={()=>setEditing(null)} onSaved={()=>{setEditing(null);onChanged();}}/>}
  </>;
}
