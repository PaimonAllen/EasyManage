import { useEffect, useId, useRef } from "react";
import type { HardwareDetails, NodeSummary } from "../types/monitoring";
import { formatDate } from "../lib/format";

const CATEGORIES: [Exclude<keyof HardwareDetails, "collected_at" | "notes">, string][] = [
  ["processors", "处理器"], ["motherboard", "主板"], ["memory", "内存"],
  ["graphics", "显卡"], ["displays", "显示器"], ["disks", "磁盘"],
  ["audio", "声卡"], ["network", "网卡"],
];

export function HardwareDetailsContent({ node }: { node: NodeSummary }) {
  const details = node.metrics?.hardware_details;
  if (!details) return <p className="hint">尚未收到设备详情。请在 Agent 管理中更新该节点 Agent；首次采集后会在这里展示。</p>;
  const stale = !node.online || Date.now() - Date.parse(details.collected_at) > 86_400_000 + 60_000;
  return <>
    <p className={`snapshot-age ${stale ? "snapshot-age--stale" : ""}`}>硬件采集于 {formatDate(details.collected_at)} · 每天更新一次{stale ? " · 历史快照" : ""}</p>
    <dl className="hardware-details-list">
      {CATEGORIES.map(([key, label]) => <div key={key}>
        <dt>{label}</dt>
        <dd>{details[key].length ? details[key].map((value, index) => <p key={index}>{value}</p>) : <p className="muted">未检测到或暂不可读取</p>}</dd>
      </div>)}
    </dl>
    {details.notes.length > 0 && <div className="hardware-details-notes"><strong>采集说明</strong><ul>{details.notes.map((note, index) => <li key={index}>{note}</li>)}</ul></div>}
  </>;
}

export function HardwareDetailsDialog({ node, onClose }: { node: NodeSummary; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  useEffect(() => {
    const element = dialog.current;
    element?.showModal();
    return () => { element?.close(); };
  }, []);
  return <dialog ref={dialog} className="hardware-details-dialog" aria-labelledby={titleId}
    onCancel={(event) => { event.preventDefault(); onClose(); }}>
    <header className="dialog-title"><h2 id={titleId}>{node.hostname} · 设备详情</h2><button type="button" className="icon-button" autoFocus onClick={onClose} aria-label="关闭设备详情">×</button></header>
    <HardwareDetailsContent node={node} />
    <div className="dialog-actions"><button type="button" className="button button--secondary button--small" onClick={onClose}>关闭</button></div>
  </dialog>;
}
