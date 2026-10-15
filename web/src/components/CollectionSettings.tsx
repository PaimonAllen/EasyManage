import { useEffect, useState } from "react";
import { fetchCollectionSettings, saveCollectionSettings, type CollectionSettingsStatus } from "../api/client";
import type { CollectionPolicy } from "../types/monitoring";
import { formatDate } from "../lib/format";
import { policyApplied } from "../lib/collection";

export function CollectionSettings() {
  const [data, setData] = useState<CollectionSettingsStatus | null>(null);
  const [draft, setDraft] = useState<CollectionPolicy | null>(null);
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  useEffect(() => {
    const controller = new AbortController();
    let pending = false;
    const refresh = async () => {
      if (pending || document.hidden) return;
      pending = true;
      try {
        const next = await fetchCollectionSettings(controller.signal);
        if (controller.signal.aborted) return;
        setData(next);
        if (!dirty) setDraft(next.policy);
        setError("");
      } catch (caught) {
        if (!controller.signal.aborted) setError(caught instanceof Error ? caught.message : "采集设置读取失败");
      } finally { pending = false; }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 30_000);
    document.addEventListener("visibilitychange", refresh);
    return () => { controller.abort(); window.clearInterval(timer); document.removeEventListener("visibilitychange", refresh); };
  }, [dirty]);

  async function save(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!draft || busy) return;
    setBusy(true); setError(""); setMessage("");
    try {
      const result = await saveCollectionSettings(draft);
      setDraft(result.policy); setDirty(false); setMessage(result.message);
      setData(await fetchCollectionSettings());
    } catch (caught) { setError(caught instanceof Error ? caught.message : "设置保存失败"); }
    finally { setBusy(false); }
  }
  return <article className="data-panel collection-settings" id="platform-collection-settings">
    <header className="panel-header"><div><h2>设置 · 采集项目与频率</h2><p>全平台统一配置，单位为秒；配置持久保存，重启不丢失。当前版本 {data?.policy.revision ?? "—"}</p></div></header>
    {error && <p className="alert" role="alert">{error}</p>}
    {message && <p className="positive hint" role="status">{message}</p>}
    {data && draft ? <form onSubmit={(event) => void save(event)}>
      <div className="table-scroll"><table className="collection-settings-table"><thead><tr><th>采集项目</th><th>具体内容 / 分组</th><th>安全范围</th><th>当前值（秒）</th><th>修改为（秒）</th></tr></thead><tbody>
        {data.items.map((item) => <tr key={item.key}>
          <td><strong>{item.title}</strong></td><td>{item.description}</td><td className="number">{item.minimum}–{item.maximum} 秒</td>
          <td className="number">{data.policy[item.key]} 秒</td>
          <td><input aria-label={`${item.title}修改为（秒）`} type="number" required min={item.minimum} max={item.maximum} step={1} disabled={busy} value={Number.isNaN(draft[item.key]) ? "" : draft[item.key]} onChange={(event) => { setDraft({ ...draft, [item.key]: event.target.valueAsNumber }); setDirty(true); setMessage(""); }} /></td>
        </tr>)}
        <tr><td><strong>设备详情</strong></td><td>处理器、主板、内存条、显卡、显示器、磁盘、声卡、物理网卡</td><td>每天一次</td><td className="number">86400 秒</td><td>固定日采集</td></tr>
      </tbody></table></div>
      <div className="collection-settings-actions">
        <button className="button button--secondary button--small" type="button" disabled={busy || !dirty} onClick={() => { setDraft(data.policy); setDirty(false); setMessage(""); }}>撤销未保存修改 / 重新加载</button>
        <button className="button button--secondary button--small" type="button" disabled={busy} onClick={() => { const defaults = { ...draft }; for (const item of data.items) defaults[item.key] = item.default_seconds; setDraft(defaults); setDirty(true); setMessage(""); }}>填入默认频率</button>
        <button className="button button--small" type="submit" disabled={busy || !dirty}>{busy ? "保存中…" : "保存并下发"}</button>
      </div>
    </form> : !error && <p className="table-empty">正在加载采集项目…</p>}
    <p className="hint">“当前值”是平台已保存的配置；各节点是否已应用请看下方状态。间隔越小，频率越高、开销越大。GPU 与磁盘不能短于完整上报间隔；缓存项目在完整上报时检查，到期后才重新采集，实际间隔会向上对齐上报周期。权限检查使用最近一次用户清单。页面刷新频率与实际采集频率不同，重复显示缓存值不算新样本。本设置不安装软件、不改变服务权限、不修改系统目录。</p>
    {data && <><h3 className="hint">节点应用状态</h3><div className="table-scroll"><table><thead><tr><th>节点</th><th>状态</th><th>已应用版本</th><th>最近心跳</th></tr></thead><tbody>
      {data.agents.map((agent) => {
        const online = Date.now() - Date.parse(agent.last_seen) <= 45_000;
        const applied = policyApplied(agent.applied_policy, data.policy);
        return <tr key={agent.node_id}><td><strong>{agent.hostname}</strong></td><td className={online && applied ? "positive" : "muted"}>{!online ? "离线 · 等待上线" : !agent.applied_policy ? "需更新 Agent" : applied ? "已应用" : "等待下次完整上报确认"}</td><td>{agent.applied_policy?.revision ?? "不支持 / 未上报"}</td><td>{formatDate(agent.last_seen)}</td></tr>;
      })}
    </tbody></table></div></>}
  </article>;
}
