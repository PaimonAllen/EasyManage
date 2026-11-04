import { NodePanel } from "../components/NodePanel";
import { useEffect, useId, useRef, useState } from "react";
import { permissionFreshSeconds } from "../lib/collection";
import { fetchPermissionReviews, markPermissionReview, type PermissionReview } from "../api/client";
import { CollectionNotes, NodeStatus } from "../components/NodeStatus";
import { PageHeading } from "../components/PageHeading";
import { formatDate } from "../lib/format";
import { samePermissionCheck } from "../lib/permissionReviews";
import type { NodeSummary, PermissionCheck } from "../types/monitoring";
import { NodeCollectionToggle, useNodeCollection } from "../components/NodeCollectionControl";

const categories = [
  { id: "home", title: "个人目录隔离" },
  { id: "shared", title: "课题组共享与临时目录" },
  { id: "project", title: "合作项目权限" },
  { id: "sudo", title: "sudo 授权与规则文件" },
  { id: "root", title: "root 与敏感文件" },
] as const;

const statusLabels: Record<PermissionCheck["status"], string> = {
  ok: "符合检查项",
  warning: "需复核",
  critical: "高风险",
  unknown: "无法确认",
  missing: "未配置",
};

const statusReasons: Record<PermissionCheck["status"], string> = {
  ok: "本项列出的静态条件符合规范。",
  warning: "当前观测未满足本项全部检查条件，需要对照规范逐项复核。",
  critical: "采集结果命中了本项高风险条件，请由管理员核对实际授权范围。",
  unknown: "现有采集信息不足以判断是否符合规范，不能当作安全通过。",
  missing: "规范要求的目录或文件未配置，无法完成本项权限检查。",
};

interface ReasonSelection {
  nodeId: string;
  hostname: string;
  collectedAt: string;
  current: boolean;
  item: PermissionCheck;
}

function PermissionReasonDialog({ selection, review, canMark, onSave, onClose }: {
  selection: ReasonSelection;
  review?: PermissionReview;
  canMark: boolean;
  onSave: (accepted: boolean, note: string) => Promise<void>;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const [note, setNote] = useState(review?.note ?? "");
  const [busy, setBusy] = useState(false);
  const [saveError, setSaveError] = useState("");
  const active = useRef(true);
  const { item } = selection;
  useEffect(() => {
    active.current = true;
    const element = dialog.current;
    element?.showModal();
    return () => { active.current = false; element?.close(); };
  }, []);
  async function save(accepted: boolean) {
    if (busy || (accepted && !canMark)) return;
    if (!window.confirm(accepted ? "确认已人工复核并接受此项当前配置？这不会修改真实权限，也不会把自动检查改为通过。" : "确认撤销此项人工 OK 标记？")) return;
    setBusy(true); setSaveError("");
    try { await onSave(accepted, note); }
    catch (caught) { if (active.current) setSaveError(caught instanceof Error ? caught.message : "标记保存失败"); }
    finally { if (active.current) setBusy(false); }
  }
  return <dialog ref={dialog} className="credentials-dialog permission-reason-dialog" aria-labelledby={titleId}
    onCancel={(event) => { event.preventDefault(); if (!busy) onClose(); }}>
    <div className="dialog-title"><div><p className="eyebrow">Permission check details</p><h2 id={titleId}>{item.title}</h2></div>
      <button className="icon-button" type="button" aria-label="关闭权限检查详情" autoFocus disabled={busy} onClick={onClose}>×</button></div>
    <p className="permission-reason-context">{selection.hostname} · <span className={`permission-status--${item.status}`}>{statusLabels[item.status]}</span></p>
    <p className="permission-reason-summary">{statusReasons[item.status]}</p>
    <dl className="permission-reason-fields">
      <div><dt>检查路径</dt><dd><code>{item.path || "未提供路径"}</code></dd></div>
      <div><dt>具体说明</dt><dd>{item.detail.trim() || "Agent 未提供进一步说明，请对照当前观测与规范要求人工复核。"}</dd></div>
      <div><dt>当前观测</dt><dd>{item.observed || "未提供观测值"}</dd></div>
      <div><dt>规范要求</dt><dd>{item.expected || "未提供规范要求"}</dd></div>
      <div><dt>采集时间</dt><dd>{formatDate(selection.collectedAt)} · {selection.current ? "打开详情时为当前快照" : "历史或过期快照，不代表当前状态"}</dd></div>
    </dl>
    <p className="hint">此处展示 Agent 上报的只读证据，不会修改权限；“需复核”或“无法确认”并不代表已证明发生越权。应由管理员核对后再决定是否修复。</p>
    {review ? <div className="permission-manual-notice"><strong>人工 OK{!canMark && " · 历史标记"}</strong><p>确认于 {formatDate(review.marked_at)}</p><p>{review.note || "未填写备注"}</p></div> : <label>复核备注（可选）<textarea value={note} onChange={(event) => setNote(event.target.value)} maxLength={1000} rows={3} disabled={busy || !canMark} placeholder="例如：该组只有本人，已核对组成员及 ACL" /></label>}
    <p className="hint">人工标记保存在平台、所有浏览器共享；原始自动结果保留。观测、规则或说明变化后自动失效。平台暂未验证操作人身份，标记不是安全认证。</p>
    {!canMark && <p className="form-error">此快照已过期、内容变化或标记接口不可用，暂不能确认人工 OK。</p>}
    {saveError && <p className="form-error" role="alert">{saveError}</p>}
    <div className="dialog-actions"><button className="button button--secondary" type="button" disabled={busy} onClick={onClose}>关闭</button>
      {review ? <button className="button button--secondary" type="button" disabled={busy} onClick={() => void save(false)}>{busy ? "保存中…" : "撤销人工标记"}</button>
        : <button className="button permission-mark-button" type="button" disabled={busy || !canMark} onClick={() => void save(true)}>{busy ? "保存中…" : "人工标记 OK"}</button>}</div>
  </dialog>;
}

function Summary({ checks, isReviewed }: { checks: PermissionCheck[]; isReviewed: (check: PermissionCheck) => boolean }) {
  const count = (status: PermissionCheck["status"]) => checks.filter((check) => check.status === status && !isReviewed(check)).length;
  return <div className="permission-summary" aria-label="权限检查结果统计">
    <span className="permission-summary--ok">{count("ok")} 符合</span>
    <span className="permission-summary--manual">{checks.filter(isReviewed).length} 人工 OK</span>
    <span className="permission-summary--warning">{count("warning")} 需复核</span>
    <span className="permission-summary--critical">{count("critical")} 高风险</span>
    <span className="permission-summary--unknown">{count("unknown")} 未知</span>
    <span className="permission-summary--missing">{count("missing")} 未配置</span>
  </div>;
}

export function PermissionChecks({ nodes }: { nodes: NodeSummary[] }) {
  const {settings,settingsError,busy,errors,toggle}=useNodeCollection("permissions");
  const enabledFor=(id:string)=>settings?.agents.find(agent=>agent.node_id===id)?.permissions_enabled;
  const [reason, setReason] = useState<ReasonSelection | null>(null);
  const [reviews, setReviews] = useState<PermissionReview[]>([]);
  const [reviewError, setReviewError] = useState("");
  const [reviewsReady, setReviewsReady] = useState(false);
  const mounted = useRef(true);
  const revision = useRef(0);
  useEffect(() => {
    mounted.current = true;
    let request: AbortController | null = null;
    async function refresh() {
      if (request || document.hidden) return;
      const startedRevision = revision.current;
      const controller = new AbortController(); request = controller;
      const timeout = window.setTimeout(() => controller.abort(), 10_000);
      try { const data = await fetchPermissionReviews(controller.signal); if (mounted.current && revision.current === startedRevision) { setReviews(data); setReviewError(""); setReviewsReady(true); } }
      catch (caught) { if (mounted.current && revision.current === startedRevision) { setReviewError(caught instanceof Error ? caught.message : "人工标记读取失败"); setReviewsReady(false); } }
      finally { window.clearTimeout(timeout); request = null; }
    }
    void refresh();
    const timer = window.setInterval(() => void refresh(), 30_000);
    const visible = () => { if (!document.hidden) void refresh(); };
    document.addEventListener("visibilitychange", visible);
    return () => { mounted.current = false; request?.abort(); window.clearInterval(timer); document.removeEventListener("visibilitychange", visible); };
  }, []);
  const reviewFor = (nodeId: string, check: PermissionCheck) => reviewsReady ? reviews.find((review) => review.node_id === nodeId && samePermissionCheck(review.check, check)) : undefined;
  const liveSelection = reason ? nodes.find((node) => node.id === reason.nodeId) : undefined;
  const selectedAudit = liveSelection?.metrics?.permission_audit;
  const canMark = Boolean(reviewsReady && liveSelection?.online && enabledFor(liveSelection.id)!==false && liveSelection.metrics?.collection_policy?.permissions_enabled!==false && selectedAudit && Date.now() - Date.parse(selectedAudit.collected_at) < 8 * 60_000
    && reason && selectedAudit.checks.some((item) => samePermissionCheck(item, reason.item)));
  async function saveReview(accepted: boolean, note: string) {
    if (!reason) return;
    revision.current += 1;
    await markPermissionReview(reason.nodeId, reason.item, accepted, note);
    try { const updated = await fetchPermissionReviews(AbortSignal.timeout(10_000)); if (mounted.current) { setReviews(updated); setReviewError(""); setReviewsReady(true); } }
    catch { if (mounted.current) { setReviewsReady(false); setReviewError("人工标记已保存，但列表刷新失败，请刷新页面"); } throw new Error("人工标记已保存，但列表刷新失败，请刷新页面"); }
  }
  return <>
    <PageHeading eyebrow="Permissions · read only" title="权限检查" description="按服务器管理操作手册核对目录入口、项目组、sudo 和 root 的只读权限快照" />
    <section className="page-content">
      {settingsError&&<p className="form-error" role="alert">{settingsError}</p>}
      <p className="hint">每台机器可独立暂停权限检查采集，重启后保留；不修改实际权限，也不影响用户状态和其他监控。恢复开启后重新检查。</p>
      {reviewError && <p className="form-error" role="alert">{reviewError}；人工 OK 暂不显示，自动结果不受影响。</p>}
      <p className="hint">检查基准：个人目录仅本人可访问；共享目录为 lab-users 组并启用 setgid/默认 ACL；合作项目使用 proj-* 组；临时目录采用文档规定的 3777。项目目录的组 rwx、setgid、其他用户无权限是依据文档隔离目标采用的检查基准，原文未指定精确 chmod。仅检查目录入口与指定配置文件，不递归扫描项目内容，也不尝试以其他用户身份访问。缺失、过期或无法读取的结果不算通过。</p>
      {nodes.map((node) => {
        const enabled=enabledFor(node.id);
        const audit = node.metrics?.permission_audit;
        const collected = audit ? Date.parse(audit.collected_at) : NaN;
        const current = enabled!==false && node.metrics?.collection_policy?.permissions_enabled!==false && node.online && Number.isFinite(collected) && Date.now() - collected < permissionFreshSeconds(node) * 1000;
        const isReviewed = (item: PermissionCheck) => Boolean(current && reviewFor(node.id, item));
        return <NodePanel node={node} key={node.id} header={
          <header className="panel-header">
            <div><h2>{node.hostname} <NodeStatus node={node} /></h2><p>权限采集于 {formatDate(audit?.collected_at)} · Agent {node.metrics?.privilege?.mode === "full" ? "Full" : "Basic"}</p></div>
            <div className="user-collection-actions"><span className={current ? "positive" : "muted"}>{enabled===false?"采集已关闭":current?"当前快照":"历史或待采集"}</span>
              <NodeCollectionToggle hostname={node.hostname} label="权限检查采集" enabled={enabled}
                applied={node.metrics?.collection_policy?.permissions_enabled} online={node.online} busy={!!busy[node.id]}
                onChange={()=>{if(enabled!==undefined){setReason(null);void toggle(node.id,enabled);}}}/>
            </div>
          </header>
          } notice={errors[node.id]&&<p className="form-error" role="alert">{errors[node.id]}</p>}>{() => <>
          {enabled===false?<p className="table-empty">权限检查采集已关闭，不展示旧快照。{node.metrics?.collection_policy?.permissions_enabled===false?"Agent 已确认停止采集。":"等待 Agent 应用；离线或旧版节点暂不能确认。"}</p>:!audit ? <p className="table-empty">尚未收到权限审计数据；需要更新平台 Server 和此节点的 Agent。不能把缺少结果当作安全通过。</p> : <>
            <Summary checks={audit.checks} isReviewed={isReviewed} />
            {audit.truncated && <p className="permission-limit">扫描超过上限：最多展示 64 个个人目录、32 个项目目录和 64 个 sudo 附加规则文件；未覆盖的路径状态未知。</p>}
            {!current && <p className="permission-limit">节点离线或权限快照超过 8 分钟，以下结果仅供历史参考。</p>}
            {categories.map((category) => {
              const checks = audit.checks.filter((item) => item.category === category.id);
              return <section className="permission-category" key={category.id}>
                <h3>{category.title} <small>{checks.length} 项</small></h3>
                {checks.length === 0 ? <p className="table-empty">本类别没有可用结果，不能判定为安全。</p> : <div className="table-scroll"><table className="permission-table"><thead><tr><th>检查项 / 路径</th><th>结果</th><th>当前观测</th><th>规范要求与说明</th></tr></thead><tbody>
                  {checks.map((item, index) => <tr key={`${item.path}-${item.title}-${index}`}>
                    <td><strong>{item.title}</strong><code>{item.path}</code></td>
                    <td>{isReviewed(item) ? <><span className="permission-manual-badge">人工 OK</span><small>自动：{statusLabels[item.status]}</small></> : <span className={`permission-status permission-status--${item.status}`}>{statusLabels[item.status]}</span>}</td>
                    <td>{item.observed}</td>
                    <td className={item.status !== "ok" ? "permission-explanation-cell" : undefined}>{item.expected}<small>{item.detail}</small>
                      {item.status !== "ok" && <button type="button" className={`permission-reason-button permission-reason-button--${isReviewed(item) ? "manual" : item.status}`}
                        aria-label={`查看 ${node.hostname} ${item.title} ${statusLabels[item.status]}的具体原因`} aria-haspopup="dialog" title="点击查看具体原因"
                        onClick={() => setReason({ nodeId: node.id, hostname: node.hostname, collectedAt: audit.collected_at, current, item })}>!</button>}
                    </td>
                  </tr>)}
                </tbody></table></div>}
              </section>;
            })}
          </>}
          {enabled!==false&&<CollectionNotes node={node} components={["permissions"]} />}
        </>}</NodePanel>;
      })}
      {!nodes.length && <p className="table-empty">尚无受管节点。</p>}
      <p className="hint">sudo 检查不执行 sudo 命令、不尝试密码、不解析所有 include/LDAP 规则；“符合检查项”只表示列出的静态条件成立，不是完整安全认证。修复权限需管理员单独评估，本页不提供自动 chmod/chown。</p>
    </section>
    {reason && <PermissionReasonDialog selection={reason} review={reviewFor(reason.nodeId, reason.item)} canMark={canMark} onSave={saveReview} onClose={() => setReason(null)} />}
  </>;
}
