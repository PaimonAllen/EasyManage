import { useEffect, useRef, useState } from "react";
import { fetchRecoveryStatus, fetchPlatformUpdateStatus, updateAgent, type OperationTarget, type RecoveryStatus, type PlatformUpdateStatus } from "../api/client";
import { AgentRestartDialog } from "../components/AgentRestartDialog";
import { ForceRestartDialog } from "../components/ForceRestartDialog";
import { PlatformUpdateDialog } from "../components/PlatformUpdateDialog";
import { NodeStatus, PrivilegeBadge } from "../components/NodeStatus";
import { PageHeading } from "../components/PageHeading";
import { formatDate } from "../lib/format";
import type { NodeSummary } from "../types/monitoring";

function updateEligible(node: NodeSummary, target?: OperationTarget): boolean {
  const privilege = node.metrics?.privilege;
  return Boolean(target?.update_available && target.update_state === "available" && node.online
    && (privilege?.mode === "basic" || (privilege?.mode === "full" && privilege.uid === 0)));
}

function restartEligible(node: NodeSummary, target?: OperationTarget): boolean {
  const privilege = node.metrics?.privilege;
  return Boolean(target?.restart_available && node.online
    && ((privilege?.mode === "basic" && privilege.uid !== 0) || (privilege?.mode === "full" && privilege.uid === 0)));
}

function updateCondition(node: NodeSummary, target?: OperationTarget): string {
  if (!target) return "未配置可信目标";
  if (!target.update_state) return "平台服务待更新";
  if (target.update_state === "not_published") return "待发布候选包";
  if (target.update_state === "untrusted_target") return "SSH 目标未就绪";
  if (target.update_state === "unknown_installed") return "当前安装摘要未知";
  if (target.update_state === "current") return "已是候选版本";
  if (!node.online) return "等待心跳恢复";
  return updateEligible(node, target) ? "有更新可分发" : "等待有效权限上报";
}

export function AgentManagement({ nodes, targets, onRefresh, onElevate }: {
  nodes: NodeSummary[];
  targets: OperationTarget[];
  onRefresh: () => void;
  onElevate: (node: NodeSummary) => void;
}) {
  const updateSection = useRef<HTMLElement>(null);
  const [selection, setSelection] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<string[]>([]);
  const [restarting, setRestarting] = useState<NodeSummary | null>(null);
  const [forceTarget, setForceTarget] = useState<{ target: OperationTarget | null } | null>(null);
  const [recovery, setRecovery] = useState<RecoveryStatus | null>(null);
  const [recoveryError, setRecoveryError] = useState("");
  const [platformUpdate, setPlatformUpdate] = useState<PlatformUpdateStatus | null>(null);
  const [platformUpdating, setPlatformUpdating] = useState(false);
  const [platformUpdateError, setPlatformUpdateError] = useState("");
  useEffect(() => {
    const controller = new AbortController();
    let pending = false;
    async function refresh() {
      if (pending) return;
      pending = true;
      try {
        const status = await fetchPlatformUpdateStatus(AbortSignal.any([controller.signal, AbortSignal.timeout(12_000)]));
        if (!controller.signal.aborted) { setPlatformUpdate(status); setPlatformUpdateError(""); }
      } catch {
        if (!controller.signal.aborted) setPlatformUpdateError("平台更新接口不可用，请先部署支持此功能的新版平台。");
      } finally { pending = false; }
    }
    void refresh();
    const timer = window.setInterval(() => void refresh(), 15_000);
    return () => { controller.abort(); window.clearInterval(timer); };
  }, []);
  useEffect(() => {
    const controller = new AbortController();
    let pending = false;
    async function refresh() {
      if (pending) return;
      pending = true;
      try {
        const status = await fetchRecoveryStatus(AbortSignal.any([controller.signal, AbortSignal.timeout(6000)]));
        if (!controller.signal.aborted) { setRecovery(status); setRecoveryError(""); }
      } catch (caught) {
        if (!controller.signal.aborted) setRecoveryError(caught instanceof Error ? caught.message : "恢复接口不可用");
      } finally { pending = false; }
    }
    void refresh();
    const timer = window.setInterval(() => void refresh(), 15_000);
    return () => { controller.abort(); window.clearInterval(timer); };
  }, []);
  const targetByNode = new Map(targets.map((target) => [target.node_id, target]));
  const available = nodes.filter((node) => updateEligible(node, targetByNode.get(node.id)));
  const selectedNodes = selection === "all" ? available : available.filter((node) => node.id === selection);
  const needsAdmin = selectedNodes.some((node) => node.metrics?.privilege?.mode === "full");
  const online = nodes.filter((node) => node.online).length;
  const full = nodes.filter((node) => node.online && node.metrics?.privilege?.mode === "full" && node.metrics.privilege.uid === 0).length;

  function selectForUpdate(nodeId: string) {
    setSelection(nodeId);
    setPassword("");
    setResults([]);
    updateSection.current?.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  async function distribute() {
    if (busy) return;
    if (!selection) { setResults(["请先选择一个节点或明确选择批量更新范围。"]); return; }
    const chosen = selectedNodes;
    if (!chosen.length) { setResults(["所选节点当前不满足安全更新条件；请检查心跳、权限和已发布候选包。"]); return; }
    const full = chosen.filter((node) => node.metrics?.privilege?.mode === "full");
    if (full.length && (!username.trim() || !password)) { setResults(["包含 Full 节点，请填写这些节点可用的管理员 SSH 用户名和密码。"]); return; }
    if (!window.confirm(`即将通过 SSH 更新并重启 ${chosen.map((node) => node.hostname).join("、")} 的 Agent，监控心跳会短暂中断。确认继续？`)) return;
    const credentials = { username: username.trim(), password };
    setPassword("");
    setBusy(true);
    setResults([]);
    const progress: string[] = [];
    for (const node of chosen) {
      try {
        const result = await updateAgent(node.id, node.metrics?.privilege?.mode === "full" ? credentials : undefined, AbortSignal.timeout(125_000));
        progress.push(`${node.hostname}：${result.message}`);
      } catch (caught) {
        progress.push(`${node.hostname}：${caught instanceof Error ? caught.message : "更新失败"}`);
      }
      setResults([...progress]);
      onRefresh();
    }
    credentials.password = "";
    setBusy(false);
  }
  return <>
    <PageHeading eyebrow="Agent fleet management" title="Agent 管理" description="管理平台与 Agent 的版本更新；查看节点心跳，并执行受控重启" />
    <section className="page-content">
      <article className="data-panel">
        <header className="panel-header"><div><h2>平台自身更新</h2><p>安装已发布的平台前后端及操作手册；保留全部 Agent 的身份和运行权限。</p></div>
          <button className="button button--small" type="button" disabled={!platformUpdate?.update_available || !platformUpdate.supported || busy || Boolean(forceTarget) || Boolean(restarting) || platformUpdating}
            title={platformUpdateError || platformUpdate?.message || "正在检查平台候选"} onClick={() => setPlatformUpdating(true)}>更新平台并重启</button></header>
        <p className="hint agent-management-hint" role="status">{platformUpdateError || platformUpdate?.message || "正在检查平台候选…"}
          {platformUpdate?.published_at && <> · 发布于 {formatDate(platformUpdate.published_at)}</>}</p>
        {platformUpdate?.last_result?.message && <p className={`hint agent-management-hint ${platformUpdate.last_result.status === "failed" ? "form-error" : ""}`}>{platformUpdate.last_result.message}</p>}
        <p className="hint agent-management-hint">先以平台普通用户运行 <code>./scripts/run-server.sh --build</code> 构建并发布候选（不会重启），再在此验证管理员并确认更新。平台将短暂离线；失败回退文件，不回退数据库迁移。</p>
      </article>
      <article className="data-panel">
        <header className="panel-header"><div><h2>应急重启</h2><p>支持数据库异常或节点离线；每次均需管理员验证。</p></div>
          <button className="button button--danger button--small" type="button" disabled={!recovery?.platform_restart_available || busy || Boolean(forceTarget) || Boolean(restarting) || platformUpdating} onClick={() => setForceTarget({ target: null })}>强制重启平台</button></header>
        <p className={`hint agent-management-hint ${recovery && !recovery.database_healthy ? "form-error" : ""}`} role="status">{recoveryError || recovery?.message || "正在检查恢复接口…"}</p>
        <div className="table-scroll"><table><thead><tr><th>可信节点</th><th>SSH 目标</th><th>应急操作</th></tr></thead><tbody>
          {targets.map((target) => <tr key={target.node_id}><td><strong>{target.hostname}</strong><small>{nodes.find((node) => node.id === target.node_id)?.online ? "心跳在线" : "心跳离线或暂不可读取"}</small></td>
            <td>{target.host}:{target.port}</td><td><button className="button button--danger button--small" type="button" disabled={!target.restart_available || busy || Boolean(forceTarget) || Boolean(restarting)} onClick={() => setForceTarget({ target })}>强制重启 Agent</button></td></tr>)}
        </tbody></table></div>
        {!targets.length && <p className="table-empty">尚未读取到可信 SSH 目标，强制重启暂不可用。</p>}
      </article>
      <article className="data-panel">
        <header className="panel-header"><div><h2>Agent 运行概览</h2><p>以节点最近一次心跳和上报的权限为准；离线快照只作历史参考。</p></div><button className="button button--secondary button--small" type="button" onClick={onRefresh}>刷新状态</button></header>
        <div className="health-grid">
          <div><strong>受管节点</strong><span>{nodes.length}</span><small>已注册 Agent</small></div>
          <div><strong>心跳在线</strong><span className="positive">{online}</span><small>{nodes.length - online} 个离线</small></div>
          <div><strong>Full Agent</strong><span>{full}</span><small>在线且报告 root / UID 0</small></div>
          <div><strong>有更新可操作</strong><span>{available.length}</span><small>候选摘要与登记的安装摘要不同</small></div>
        </div>
        <div className="table-scroll"><table><thead><tr><th>节点 / Agent</th><th>最近心跳</th><th>运行权限</th><th>可信 SSH 目标</th><th>更新条件</th><th>管理操作</th></tr></thead><tbody>
          {nodes.map((node) => {
            const target = targetByNode.get(node.id);
            const privileged = node.metrics?.privilege?.mode === "full" && node.metrics.privilege.uid === 0;
            const canUpdate = updateEligible(node, target);
            const canRestart = restartEligible(node, target);
            return <tr key={node.id}>
              <td><strong>{node.hostname}</strong> <NodeStatus node={node} /><small>Agent {node.agent_version || "未知"}</small></td>
              <td className={node.online ? "positive" : "muted"}>{formatDate(node.last_seen)}<small>{node.online ? "心跳正常" : "心跳已过期"}</small></td>
              <td><PrivilegeBadge node={node} /></td>
              <td>{target ? <><strong>{target.default_user}@{target.host}</strong><small>SSH 端口 {target.port}</small></> : "未配置"}</td>
              <td className={canUpdate ? "positive" : "muted"}>{updateCondition(node, target)}</td>
              <td><div className="agent-management-actions">
                {node.online && !privileged && node.metrics?.privilege?.mode === "basic" && target?.elevation_available && <button className="button button--secondary button--small" type="button" onClick={() => onElevate(node)}>提权 Full</button>}
                <button className="button button--secondary button--small" type="button" disabled={!canRestart || busy} title={!target?.restart_available ? "需要新版平台和可信 SSH 目标" : !node.online ? "等待在线心跳" : "只重启 Agent 服务"} onClick={() => setRestarting(node)}>重启</button>
                <button className="button button--secondary button--small" type="button" disabled={!canUpdate || busy} title={canUpdate ? "更新候选包并重启 Agent" : updateCondition(node, target)} onClick={() => selectForUpdate(node.id)}>更新并重启</button>
              </div></td>
            </tr>;
          })}
        </tbody></table></div>
        {!nodes.length && <p className="table-empty">尚未收到任何节点的 Agent 注册。</p>}
        <p className="hint agent-management-hint">“有更新”依据可信清单中的候选摘要与登记的安装摘要比较，不是直接读取目标机二进制；身份、摘要和服务状态仍由服务端再次校验。重启或更新均会短暂中断 Agent 心跳，不会重启主机。没有提供停止、降权或卸载按钮。节点互联和平台性能详情见「平台」。</p>
      </article>
      <article className="data-panel" ref={updateSection}>
        <header className="panel-header"><div><h2>更新并重启</h2><p>仅候选摘要与已登记安装摘要不同的节点可选；确认后逐台更新。</p></div></header>
        <p className="hint agent-management-hint">先在主节点以普通用户运行 <code>./scripts/run-server.sh --build</code> 发布版本。Basic 节点使用已有普通用户 SSH 密钥；Full 节点每次更新都验证管理员 SSH 与 sudo 密码。更新结果以新版本心跳确认。</p>
      <div className="admin-form agent-management-form">
        <label>分发范围<select value={selection} onChange={(event) => { setSelection(event.target.value); setPassword(""); setResults([]); }} disabled={busy}><option value="">请选择节点</option>{available.map((node) => <option key={node.id} value={node.id}>{node.hostname}</option>)}<option value="all" disabled={!available.length}>全部可更新节点（{available.length}）</option></select></label>
        {needsAdmin && <label>Full 节点管理员用户名<input value={username} onChange={(event) => setUsername(event.target.value)} disabled={busy} autoComplete="off" placeholder="例如 cvpr" /></label>}
        {needsAdmin && <label>Full 节点管理员密码<input type="password" value={password} onChange={(event) => setPassword(event.target.value)} disabled={busy} autoComplete="off" /></label>}
        <button className="button" type="button" onClick={() => void distribute()} disabled={busy || !selectedNodes.length}>{busy ? "正在逐台更新…" : "确认更新并重启"}</button>
      </div>
      <div className="agent-management-results" role="status" aria-live="polite">{results.map((result, index) => <p key={`${index}-${result}`}>{result}</p>)}</div>
      <p className="hint agent-management-hint">多个 Full 节点若使用不同管理员凭据，请逐台更新。页面不会在没有确认的情况下执行分发；失败时保留逐台结果，最终以 Agent 心跳为准。</p>
      </article>
    </section>
    {restarting && <AgentRestartDialog node={restarting} onClose={() => setRestarting(null)} onSubmitted={onRefresh} />}
    {forceTarget && <ForceRestartDialog target={forceTarget.target} onClose={() => setForceTarget(null)} onSubmitted={onRefresh} />}
    {platformUpdating && platformUpdate && <PlatformUpdateDialog candidate={platformUpdate} onClose={() => { setPlatformUpdating(false); onRefresh(); }} />}
  </>;
}
