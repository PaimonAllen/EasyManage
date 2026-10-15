import { useEffect, useRef, useState } from "react";
import { restartAgent } from "../api/client";
import type { NodeSummary } from "../types/monitoring";

export function AgentRestartDialog({ node, onClose, onSubmitted }: {
  node: NodeSummary;
  onClose: () => void;
  onSubmitted: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const active = useRef(true);
  const full = node.metrics?.privilege?.mode === "full" && node.metrics.privilege.uid === 0;
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");

  useEffect(() => {
    active.current = true;
    dialog.current?.showModal();
    return () => { active.current = false; };
  }, []);

  return <dialog className="credentials-dialog" ref={dialog} onCancel={(event) => { if (busy) event.preventDefault(); else onClose(); }}>
    <form onSubmit={(event) => {
      event.preventDefault();
      if (busy) return;
      setBusy(true);
      setError("");
      const credentials = full ? { username: username.trim(), password } : undefined;
      const request = restartAgent(node.id, credentials, AbortSignal.timeout(85_000));
      setPassword("");
      void request.then((result) => {
        if (!active.current) return;
        setMessage(result.message || "Agent 重启请求已执行，等待新心跳确认。");
        onSubmitted();
      }).catch((caught: unknown) => {
        if (active.current) setError(caught instanceof Error ? caught.message : "重启失败");
      }).finally(() => {
        if (active.current) setBusy(false);
      });
    }}>
      <div className="dialog-title"><div><p className="eyebrow">Agent service restart</p><h2>重启 {node.hostname} Agent</h2></div><button className="icon-button" type="button" aria-label="关闭" disabled={busy} onClick={onClose}>×</button></div>
      <p className="hint">只重启此节点的 EasyManage Agent 服务，不重启主机。生产监控心跳会短暂中断；离线或权限模式不确定时服务端会拒绝操作。</p>
      {message ? <div className="notice notice--success" role="status">{message}<p>最终状态以新心跳为准。</p></div> : <>
        {full && <>
          <label>管理员用户名<input required autoFocus value={username} onChange={(event) => setUsername(event.target.value)} disabled={busy} autoComplete="off" maxLength={64} placeholder="例如 cvpr" /></label>
          <label>管理员密码<input required type="password" value={password} onChange={(event) => setPassword(event.target.value)} disabled={busy} autoComplete="off" maxLength={1024} /></label>
          <p className="hint">Full Agent 每次重启都验证目标机管理员 SSH 与 sudo，密码不保存在浏览器。</p>
        </>}
        {!full && <p className="hint">Basic Agent 将通过可信普通用户 SSH 目标重启，不需要管理员密码。</p>}
      </>}
      {error && <p className="form-error" role="alert">{error}</p>}
      <div className="dialog-actions"><button className="button button--secondary" type="button" disabled={busy} onClick={onClose}>{message ? "关闭" : "取消"}</button>{!message && <button className="button" type="submit" disabled={busy || (full && (!username.trim() || !password))}>{busy ? "正在重启…" : "确认重启 Agent"}</button>}</div>
    </form>
  </dialog>;
}
