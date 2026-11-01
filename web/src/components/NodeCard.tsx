import { useId, useState } from "react";
import { NodeCharts } from "./NodeCharts";
import { HardwareDetailsDialog } from "./HardwareDetailsDialog";
import {cpuSockets} from "../lib/cpu";
import {gpuDevices} from "../lib/gpu";
import {
  CollectionNotes,
  NodeStatus,
  PrivilegeBadge,
  SnapshotAge,
} from "./NodeStatus";
import type { LiveNode, MetricsSnapshot, NodeMetricHistory, NodeSummary } from "../types/monitoring";

export function NodeCard({
  node,
  history,
  historyError,
  liveSamples,
  temperatureSnapshots,
  checkedAt,
  onElevate,
  elevationAvailable,
}: {
  node: NodeSummary;
  history?: NodeMetricHistory;
  historyError?: string;
  liveSamples: LiveNode[];
  temperatureSnapshots: MetricsSnapshot[];
  checkedAt: number;
  onElevate: () => void;
  elevationAvailable: boolean;
}) {
  const metrics = node.metrics;
  const [showDetails, setShowDetails] = useState(false);
  const [showGpus, setShowGpus] = useState(true);
  const [showCpus,setShowCpus]=useState(true);
  const cpus=cpuSockets(metrics?.cpu_sockets);
  const gpus=gpuDevices(metrics?.gpus??[]);
  const cpuDetailsId=`cpu-details-${useId().replace(/[^a-zA-Z0-9_-]/g,"")}`;
  const gpuDetailsId = `gpu-details-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;

  return (
    <article className={`node-card ${node.online ? "" : "node-card--offline"}`}>
      <header className="node-card__header">
        <div className="node-card__identity">
          <div className="node-card__title-row">
            <h2>{node.hostname}</h2>
            <NodeStatus node={node} />
          </div>
          <p>
            {node.os_name} {node.os_version} · {node.architecture} · Agent{" "}
            {node.agent_version}
          </p>
          <SnapshotAge node={node} />
        </div>
        <div className="node-card__actions">
          <a className="button button--small button--secondary" href={`/usage-history?node=${encodeURIComponent(node.id)}`} target="_blank" rel="noopener noreferrer">历史使用记录 ↗</a>
          <button type="button" className="button button--small button--secondary" onClick={() => setShowDetails(true)}>设备详情</button>
          {cpus.length>1&&<button type="button" className="button button--small button--secondary" aria-expanded={showCpus} aria-controls={cpuDetailsId} onClick={()=>setShowCpus(value=>!value)}>{showCpus?"收起 CPU":`展开所有 CPU（${cpus.length}）`}</button>}
          {gpus.length>1&&<button type="button" className="button button--small button--secondary" aria-expanded={showGpus} aria-controls={gpuDetailsId} onClick={()=>setShowGpus(value=>!value)}>{showGpus?"收起 GPU":`展开所有 GPU（${gpus.length}）`}</button>}
          <PrivilegeBadge node={node} />
          <button
            className="button button--small button--secondary"
            disabled={
              !node.online ||
              !elevationAvailable ||
              (metrics?.privilege?.mode === "full" && metrics.privilege.uid === 0)
            }
            onClick={onElevate}
            title={
              !elevationAvailable ? "该节点尚未配置 SSH 提权目标" : undefined
            }
          >
            {metrics?.privilege?.mode === "full" && metrics.privilege.uid === 0
              ? "已获得 Full 权限"
              : "提升到 Full"}
          </button>
        </div>
      </header>
      {!node.online && (
        <p className="stale-note">{node.monitoring_paused ? "已请求暂停监控；确认状态见机器管理。下方仅保留历史采集结果，恢复请点“恢复监控”。" : "节点离线，下方保留最后一次采集结果。"}</p>
      )}

      {!metrics ? (
        <div className="empty-state">节点已经注册，正在等待第一组指标。</div>
      ) : (
        <NodeCharts node={node} history={history} historyError={historyError} liveSamples={liveSamples} temperatureSnapshots={temperatureSnapshots} checkedAt={checkedAt} gpuExpanded={showGpus} gpuDetailsId={gpuDetailsId} cpuExpanded={showCpus} cpuDetailsId={cpuDetailsId}/>
      )}
      <CollectionNotes node={node} />
      <footer>
        {node.cpu_model} · Kernel {node.kernel_version}
      </footer>
      {showDetails && <HardwareDetailsDialog node={node} onClose={() => setShowDetails(false)} />}
    </article>
  );
}
