import { useEffect, useRef, useState } from "react";
import { fetchAdminCredentials, forgetAdminCredentials, saveAdminCredentials, type AdminAuthentication } from "../api/client";
import { verifyAdminCredentialScopes } from "../lib/adminCredentialScope";

export function useAdminCredentials(nodeIds: (string | null)[]) {
  const scope = JSON.stringify(nodeIds);
  const [checkedScope, setCheckedScope] = useState("");
  const [available, setAvailable] = useState(false);
  const [mode, setMode] = useState<"saved" | "password" | "key">("password");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [key, setKey] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [sudoPassword, setSudoPassword] = useState("");
  const [error, setError] = useState("");
  const keyRead = useRef(0);
  useEffect(() => {
    keyRead.current += 1;
    const controller = new AbortController();
    setAvailable(false); setMode("password"); setPassword(""); setKey(""); setPassphrase(""); setSudoPassword(""); setError("");
    const ids = JSON.parse(scope) as (string|null)[];
    void Promise.all(ids.map(id => fetchAdminCredentials(id, controller.signal))).then(results => {
      if (controller.signal.aborted) return;
      const usable = results.length > 0 && results.every(result => result.available);
      setAvailable(usable); setMode(usable ? "saved" : "password"); setCheckedScope(scope);
    }).catch(() => {
      if (!controller.signal.aborted) { setCheckedScope(scope); setError("无法读取管理员凭据状态，请确认新版平台后端已部署。"); }
    });
    return () => { controller.abort(); keyRead.current += 1; };
  }, [scope]);
  const ready = checkedScope === scope && nodeIds.length > 0 && (mode === "saved" ? available : Boolean(username.trim() && (mode === "key" ? key : password)));
  function clear() { keyRead.current += 1; setPassword(""); setKey(""); setPassphrase(""); setSudoPassword(""); }
  function changeMode(value: "saved" | "password" | "key") { clear(); setError(""); setMode(value); }
  async function readKey(file?: File) {
    const generation=++keyRead.current;
    setKey(""); setError("");
    if (!file) return;
    if (file.size>12*1024) { setError("私钥文件超过 12 KiB"); return; }
    try {
      const value=await file.text();
      if (keyRead.current!==generation) return;
      if (!value.startsWith("-----BEGIN ") || value.includes("\0")) setError("请选择 SSH 私钥，不是 .pub 公钥");
      else setKey(value);
    } catch { if (keyRead.current===generation) setError("无法读取私钥文件"); }
  }
  async function prepare(): Promise<AdminAuthentication> {
    if (!ready) throw new Error("请先完成管理员鉴权信息。");
    const credentials: AdminAuthentication = mode === "saved" ? {use_saved:true} : {
      username: username.trim(), password:mode === "password" ? password : "",
      private_key:mode === "key" ? key : "", passphrase:mode === "key" ? passphrase : "", sudo_password:sudoPassword,
    };
    clear();
    try {
      // Only fixed read-only root verification runs here. The explicit operation follows it.
      await verifyAdminCredentialScopes(nodeIds,
        id => saveAdminCredentials(id, credentials), id => fetchAdminCredentials(id));
      setAvailable(true); setMode("saved");
      return {use_saved:true};
    } finally {
      if (!("use_saved" in credentials)) { credentials.password=""; credentials.private_key=""; credentials.passphrase=""; credentials.sudo_password=""; }
    }
  }
  async function forget() {
    setError("");
    try { for (const id of nodeIds) await forgetAdminCredentials(id); setAvailable(false); setMode("password"); clear(); }
    catch (caught) { setError(caught instanceof Error ? caught.message : "清除失败"); }
  }
  return {ready, loading:checkedScope!==scope, available, mode, setMode:changeMode, username, setUsername,
    password, setPassword, key, passphrase, setPassphrase, sudoPassword, setSudoPassword, error, setError, prepare, forget, readKey};
}

export function AdminCredentialFields({auth, disabled}: {auth: ReturnType<typeof useAdminCredentials>; disabled: boolean}) {
  const [forgetting, setForgetting] = useState(false);
  const locked = disabled || auth.loading || forgetting;
  return <fieldset disabled={locked} className="admin-credentials">
    <legend>管理员鉴权</legend>
    {auth.loading ? <p className="hint">正在检查已保存的授权…</p> : <>
      <label>鉴权方式<select value={auth.mode} onChange={event => auth.setMode(event.target.value as typeof auth.mode)}>
        {auth.available && <option value="saved">使用平台已加密保存的凭据</option>}
        <option value="password">SSH 密码</option><option value="key">SSH 私钥</option>
      </select></label>
      {auth.mode !== "saved" ? <>
        <label>管理员 SSH 用户名<input required autoComplete="off" maxLength={64} value={auth.username} onChange={event=>auth.setUsername(event.target.value)} /></label>
        {auth.mode === "password" ? <label>SSH 密码<input required type="password" autoComplete="off" maxLength={1024} value={auth.password} onChange={event=>auth.setPassword(event.target.value)} /></label> : <>
          <label>SSH 私钥文件（OpenSSH / PEM，最多 12 KiB）<input type="file" onChange={event=>{
            const file=event.target.files?.[0]; event.target.value=""; void auth.readKey(file);
          }}/></label>
          {auth.key && <p className="hint">私钥已选择，内容不会显示。</p>}
          <label>私钥口令（如有）<input type="password" autoComplete="off" maxLength={1024} value={auth.passphrase} onChange={event=>auth.setPassphrase(event.target.value)} /></label>
        </>}
        <label>sudo 密码（如与 SSH 密码不同）<input type="password" autoComplete="off" maxLength={1024} value={auth.sudoPassword} onChange={event=>auth.setSudoPassword(event.target.value)} /></label>
        <p className="hint">首次验证 SSH 和管理员权限后加密保存在平台；按所选机器复用，不会授权其他机器。私钥登录时，如 sudo 需要密码请填写。</p>
      </> : <p className="hint">将复用平台保险库中的凭据，无需再次输入密码或私钥。</p>}
      {auth.available && <button type="button" className="button button--secondary button--small" onClick={()=>{
        if (!window.confirm("清除所选机器的管理员凭据？不影响普通用户 SSH 长连接。")) return;
        setForgetting(true); void auth.forget().finally(()=>setForgetting(false));
      }}>清除已保存的管理员凭据</button>}
    </>}
    <p className="hint">凭据不返回前端、不写入浏览器存储或日志。当前浏览器授权有效期 30 天，成功使用会续期；换浏览器、清除授权或凭据失效时需重新鉴权。</p>
    {auth.error && <p className="form-error" role="alert">{auth.error}</p>}
  </fieldset>;
}
