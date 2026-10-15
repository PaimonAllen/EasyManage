import { useState } from "react";
import { NodeCharts } from "./NodeCharts";
import { HardwareDetailsDialog } from "./HardwareDetailsDialog";
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
          <button type="button" className="button button--small button--secondary" onClick={() => setShowDetails(true)}>设备详情</button>
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
        <p className="stale-note">节点离线，下方保留最后一次采集结果。</p>
      )}

      {!metrics ? (
        <div className="empty-state">节点已经注册，正在等待第一组指标。</div>
      ) : (
        <NodeCharts node={node} history={history} historyError={historyError} liveSamples={liveSamples} temperatureSnapshots={temperatureSnapshots} checkedAt={checkedAt} />
      )}
      <CollectionNotes node={node} />
      <footer>
        {node.cpu_model} · Kernel {node.kernel_version}
      </footer>
      {showDetails && <HardwareDetailsDialog node={node} onClose={() => setShowDetails(false)} />}
    </article>
  );
}
