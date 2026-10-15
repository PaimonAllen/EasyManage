import { useState } from "react";
import { manageUser, type ManagedUser, type OperationTarget, type UserOperation } from "../api/client";
import { accountState } from "../lib/accountStatus";

export function UserManagement({ targets }: { targets: OperationTarget[] }) {
  const [nodeId, setNodeId] = useState(targets[0]?.node_id ?? "");
  const [action, setAction] = useState<UserOperation["action"]>("list");
  const [actor, setActor] = useState("");
  const [password, setPassword] = useState("");
  const [subject, setSubject] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [shell, setShell] = useState("");
  const [groupsText, setGroupsText] = useState("");
  const [changeGroups, setChangeGroups] = useState(false);
  const [confirmation, setConfirmation] = useState("");
  const [accounts, setAccounts] = useState<ManagedUser[] | null>(null);
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (busy) return;
    const chosen = nodeId || targets[0]?.node_id;
    if (!chosen) { setMessage("没有配置可信 SSH 目标"); return; }
    const request: UserOperation = {
      username: actor.trim(), password, action, subject: action === "list" ? "" : subject.trim(),
      display_name: displayName.trim(), shell,
      confirmation: action === "list" ? "" : confirmation.trim(),
    };
    if (action === "create" || (action === "update" && changeGroups)) {
      request.groups = groupsText.split(",").map((item) => item.trim()).filter(Boolean);
    }
    setPassword("");
    setBusy(true);
    setMessage("");
    try {
      const result = await manageUser(chosen, request, AbortSignal.timeout(50_000));
      if (action === "list") setAccounts(result.accounts ?? []);
      else { setAccounts(null); setMessage(result.message ?? "目标节点已执行操作；请重新鉴权读取清单。"); }
    } catch (caught) {
      setMessage(caught instanceof Error ? caught.message : "账户操作失败");
    } finally {
      request.password = "";
      setBusy(false);
    }
  }
  return <article className="data-panel">
    <header className="panel-header"><div><h2>管理员账户管理</h2><p>每次读取、创建、修改或删除均向目标机重新验证管理员 SSH 和 sudo 密码。</p></div></header>
    <form className="admin-form" onSubmit={(event) => void submit(event)}>
      <label>目标节点<select value={nodeId || targets[0]?.node_id || ""} onChange={(event) => { setNodeId(event.target.value); setAccounts(null); }} disabled={busy}>{targets.map((target) => <option key={target.node_id} value={target.node_id}>{target.hostname}</option>)}</select></label>
      <label>操作<select value={action} onChange={(event) => { setAction(event.target.value as UserOperation["action"]); setAccounts(null); }} disabled={busy}><option value="list">读取账户</option><option value="create">创建账户</option><option value="update">修改账户</option><option value="delete">删除账户</option></select></label>
      <label>管理员用户名<input required value={actor} onChange={(event) => setActor(event.target.value)} autoComplete="off" placeholder="例如 cvpr" disabled={busy} /></label>
      <label>管理员密码<input required type="password" value={password} onChange={(event) => setPassword(event.target.value)} autoComplete="off" disabled={busy} /></label>
      {action !== "list" && <label>目标账户<input required value={subject} onChange={(event) => setSubject(event.target.value)} autoComplete="off" disabled={busy} /></label>}
      {(action === "create" || action === "update") && <>
        <label>显示名称<input value={displayName} onChange={(event) => setDisplayName(event.target.value)} maxLength={100} disabled={busy} /></label>
        <label>登录 Shell<select value={shell} onChange={(event) => setShell(event.target.value)} disabled={busy}><option value="">{action === "create" ? "默认 /bin/bash" : "保留原设置"}</option><option value="/bin/bash">/bin/bash</option><option value="/usr/bin/zsh">/usr/bin/zsh</option><option value="/usr/sbin/nologin">/usr/sbin/nologin</option></select></label>
        {action === "update" && <label className="checkbox-label"><input type="checkbox" checked={changeGroups} onChange={(event) => setChangeGroups(event.target.checked)} disabled={busy} /> 修改辅助组</label>}
        {(action === "create" || changeGroups) && <label>辅助组（逗号分隔）<input value={groupsText} onChange={(event) => setGroupsText(event.target.value)} placeholder="lab-users,gpu-users" disabled={busy} /></label>}
      </>}
      {action !== "list" && <label>再次输入目标账户名确认<input required value={confirmation} onChange={(event) => setConfirmation(event.target.value)} disabled={busy} /></label>}
      <button className="button" disabled={busy || !targets.length}>{busy ? "正在验证和执行…" : action === "list" ? "验证并读取" : "验证并执行"}</button>
    </form>
    <p className="hint">只管理普通 UID 范围账户；root、管理员、正在运行进程的账户受到保护。删除账户保留 Home 文件，不主动终止进程。新账户默认没有可用密码，需要管理员单独设置登录方式。</p>
    {message && <p role="status" className="notice">{message}</p>}
    {accounts && <div className="table-scroll"><table><thead><tr><th>账户</th><th>UID</th><th>显示名称</th><th>Shell</th><th>状态</th></tr></thead><tbody>{accounts.map((item) => <tr key={item.username}><td>{item.username}</td><td>{item.uid}</td><td>{item.display_name || "—"}</td><td><code>{item.shell}</code></td><td title="仅核验登录 Shell；不代表密码、密钥或 SSH 策略可用"><span className={accountState(item.account_status).className}>{accountState(item.account_status).label}</span></td></tr>)}</tbody></table></div>}
  </article>;
}
