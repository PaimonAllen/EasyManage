import { useEffect, useRef, useState } from "react";
import { forceRestartService, type OperationTarget } from "../api/client";

export function ForceRestartDialog({ target, onClose, onSubmitted }: {
  target: OperationTarget | null;
  onClose: () => void;
  onSubmitted: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const mounted = useRef(true);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const label = target ? `${target.hostname} Agent` : "平台";
  useEffect(() => {
    mounted.current = true;
    dialog.current?.showModal();
    return () => { mounted.current = false; dialog.current?.close(); };
  }, []);
  async function submit() {
    if (busy) return;
    setBusy(true);
    setError("");
    const credentials = { username: username.trim(), password };
    setPassword("");
    try {
      const result = await forceRestartService(target?.node_id ?? null, credentials, AbortSignal.timeout(75_000));
      if (mounted.current) { setMessage(result.message); onSubmitted(); }
    } catch (caught) {
      if (mounted.current) setError(caught instanceof Error ? caught.message : "连接中断，请检查实际服务状态后再操作");
    } finally {
      credentials.password = "";
      if (mounted.current) setBusy(false);
    }
  }
  return <dialog ref={dialog} className="credentials-dialog" aria-label={`强制重启${label}`}
    onCancel={(event) => { if (busy) event.preventDefault(); else onClose(); }}>
    <form onSubmit={(event) => { event.preventDefault(); void submit(); }}>
      <div className="dialog-title"><h2>强制重启 {label}</h2><button type="button" className="icon-button" disabled={busy} onClick={onClose} aria-label="关闭">×</button></div>
      {message ? <div className="notice notice--success" role="status">{message}{!target && <p>等待约 5 秒后刷新页面检查平台状态。</p>}</div> : <>
        <p className="form-error">将立即终止该服务的全部进程并重新启动，当前请求及监控连接会中断。</p>
        <p className="hint">{target ? "支持离线或故障 Agent；保留目标机已配置的 Full / Basic 权限。" : "仅重启平台服务；数据库故障仍需管理员恢复 PostgreSQL。"} 每次操作均需目标服务器的管理员 SSH 与 sudo 验证。</p>
        {!target&&<p className="hint">平台重启会短暂中断 SSH 长连接；已加密保存凭据且启用的连接将自动恢复。不影响 Agent 安装和原有权限。</p>}
        <label>管理员用户名<input required autoFocus value={username} disabled={busy} onChange={(event) => setUsername(event.target.value)} maxLength={64} autoComplete="off" placeholder="例如 cvpr" /></label>
        <label>管理员密码<input required type="password" value={password} disabled={busy} onChange={(event) => setPassword(event.target.value)} maxLength={1024} autoComplete="off" /></label>
      </>}
      {error && <p className="form-error" role="alert">{error}</p>}
      <div className="dialog-actions"><button className="button button--secondary" type="button" disabled={busy} onClick={onClose}>{message ? "关闭" : "取消"}</button>
        {!message && <button className="button button--danger" type="submit" disabled={busy || !username.trim() || !password}>{busy ? "正在验证并重启…" : "确认强制重启"}</button>}
        {message && !target && <button className="button" type="button" onClick={() => window.location.reload()}>刷新页面</button>}
      </div>
    </form>
  </dialog>;
}
