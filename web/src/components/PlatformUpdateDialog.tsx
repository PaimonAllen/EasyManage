import { useEffect, useRef, useState } from "react";
import { fetchPlatformUpdateStatus, updatePlatform, type PlatformUpdateStatus } from "../api/client";
import { formatDate } from "../lib/format";

export function PlatformUpdateDialog({candidate, onClose}: {candidate: PlatformUpdateStatus; onClose: () => void}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const mounted = useRef(true);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [finished, setFinished] = useState(false);
  useEffect(() => {
    mounted.current = true;
    dialog.current?.showModal();
    return () => { mounted.current = false; dialog.current?.close(); };
  }, []);
  useEffect(() => {
    if (!submitted) return;
    const controller = new AbortController();
    const started = Date.now();
    let pending = false;
    const timer = window.setInterval(() => void check(), 2000);
    async function check() {
      if (pending) return;
      if (Date.now() - started > 250_000) {
        window.clearInterval(timer);
        setBusy(false);
        setError("等待更新结果超时，不能确认成功。请检查平台服务日志及更新结果后再操作，勿重复提交。");
        return;
      }
      pending = true;
      try {
        const result = await fetchPlatformUpdateStatus(AbortSignal.any([controller.signal, AbortSignal.timeout(10_000)]));
        if (controller.signal.aborted) return;
        const outcome = result.last_result;
        if (outcome && outcome.sha256 === candidate.candidate_sha256) {
          if (outcome.status === "scheduled") setMessage("平台更新已安排，等待安装任务启动…");
          else if (outcome.status === "installing") setMessage("正在安装并重启平台；等待 HTTPS 和数据库就绪检查…");
          else {
            window.clearInterval(timer);
            setBusy(false);
            setFinished(outcome.status === "success");
            if (outcome.status === "success") setMessage(outcome.message || "平台更新完成并恢复就绪。");
            else setError(outcome.message || "平台更新失败，请检查回退结果与服务日志。");
          }
        }
      } catch {
        if (!controller.signal.aborted) setMessage("平台连接暂时中断，正在等待恢复并核实更新结果…");
      } finally { pending = false; }
    }
    return () => { controller.abort(); window.clearInterval(timer); };
  }, [submitted, candidate.candidate_sha256]);
  async function submit() {
    if (busy || submitted || !candidate.candidate_sha256 || !confirmed) return;
    const credentials = {username: username.trim(), password};
    setPassword("");
    setBusy(true);
    setError("");
    try {
      const result = await updatePlatform(credentials, candidate.candidate_sha256, AbortSignal.timeout(75_000));
      if (mounted.current) { setMessage(result.message); setSubmitted(true); }
    } catch (caught) {
      if (!mounted.current) return;
      // A lost response is not proof that scheduling failed. Never automatically retry a mutation.
      const text = caught instanceof Error ? caught.message : "提交失败";
      if (caught instanceof TypeError || (caught instanceof DOMException && caught.name === "TimeoutError")) {
        setMessage("提交响应中断，正在检查平台是否已开始更新；不会重复提交。");
        setSubmitted(true);
      } else { setError(text); setBusy(false); }
    } finally { credentials.password = ""; }
  }
  return <dialog ref={dialog} className="credentials-dialog" aria-label="更新平台并重启"
    onCancel={(event) => { if (busy) event.preventDefault(); else onClose(); }}>
    <form onSubmit={(event) => { event.preventDefault(); void submit(); }}>
      <div className="dialog-title"><h2>更新平台并重启</h2><button className="icon-button" type="button" disabled={busy} onClick={onClose} aria-label="关闭">×</button></div>
      <p className="hint">更新平台后端、前端和操作手册；不更新 Agent，不改变 Full / Basic 权限。</p>
      <p className="hint">候选发布于 {formatDate(candidate.published_at || "")}<br />SHA-256：<code style={{overflowWrap: "anywhere"}}>{candidate.candidate_sha256}</code></p>
      {!submitted && <>
        <p className="form-error">平台将短暂离线。失败时回退平台文件，但已执行的数据库迁移不能自动回退。</p>
        <label>平台服务器管理员用户名<input required autoFocus autoComplete="off" maxLength={64} disabled={busy} value={username} onChange={(event) => setUsername(event.target.value)} /></label>
        <label>管理员密码<input required type="password" autoComplete="off" maxLength={1024} disabled={busy} value={password} onChange={(event) => setPassword(event.target.value)} /></label>
        <label className="machine-confirm"><input type="checkbox" disabled={busy} checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} /> 我已确认候选版本、数据库备份和短暂离线风险</label>
      </>}
      {message && <div className={`notice ${finished ? "notice--success" : ""}`} role="status">{message}</div>}
      {error && <p className="form-error" role="alert">{error}</p>}
      <div className="dialog-actions"><button className="button button--secondary" type="button" disabled={busy} onClick={onClose}>{submitted ? "关闭" : "取消"}</button>
        {!submitted && <button className="button" type="submit" disabled={busy || !username.trim() || !password || !confirmed}>{busy ? "正在验证并安排更新…" : "确认更新平台并重启"}</button>}
        {finished && <button className="button" type="button" onClick={() => window.location.reload()}>载入新版平台</button>}
      </div>
    </form>
  </dialog>;
}
