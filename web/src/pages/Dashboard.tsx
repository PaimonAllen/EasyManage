import { useEffect, useState } from "react";
import { NodeCard } from "../components/NodeCard";
import { MachineManagement } from "../components/MachineManagement";
import { PageHeading } from "../components/PageHeading";
import { useMetricHistory } from "../hooks/useMetricHistory";
import { useLiveMetrics } from "../hooks/useLiveMetrics";
import { recordTemperatureSnapshots } from "../lib/temperature";
import type { OperationTarget } from "../api/client";
import type { MetricsSnapshot, NodeListResponse, NodeSummary } from "../types/monitoring";

export function Dashboard({
  data,
  targets,
  onElevate,
  onRefresh,
}: {
  data: NodeListResponse | null;
  targets: OperationTarget[];
  onElevate: (node: NodeSummary) => void;
  onRefresh: () => void;
}) {
  const online = data?.nodes.filter((node) => node.online).length ?? 0;
  const paused = data?.nodes.filter((node) => node.monitoring_paused).length ?? 0;
  const { history, errors } = useMetricHistory(data?.nodes.map((node) => node.id) ?? []);
  const live = useLiveMetrics();
  const [temperatureSnapshots, setTemperatureSnapshots] = useState<Record<string, MetricsSnapshot[]>>({});
  useEffect(() => {
    if (data) setTemperatureSnapshots(previous => recordTemperatureSnapshots(previous, data.nodes, Date.parse(data.generated_at)));
  }, [data]);
  return (
    <div className="overview-page">
      <PageHeading
        eyebrow="Infrastructure overview"
        title="节点运行概览"
        description="近 10 分钟 · 按平台配置采集，默认 CPU / 内存 / 网络每秒采样 · 缓存值不作为新样本"
      >
        <div className="summary">
          <div>
            <strong>{data?.nodes.length ?? "—"}</strong>
            <span>节点总数</span>
          </div>
          <div>
            <strong className="online-number">{data ? online : "—"}</strong>
            <span>当前在线</span>
          </div>
          <div>
            <strong>{data ? data.nodes.length - online - paused : "—"}</strong>
            <span>当前离线</span>
          </div>
          {paused>0&&<div><strong>{paused}</strong><span>监控暂停</span></div>}
        </div>
      </PageHeading>
      <MachineManagement onRefresh={onRefresh}/>
      {live.error && <p className="history-warning">实时指标暂不可用，仍显示常规采样：{live.error}</p>}
      <section className="nodes">
        {data?.nodes.map((node) => (
          <NodeCard
            key={node.id}
            node={node}
            history={history[node.id]}
            historyError={errors[node.id]}
            liveSamples={live.samples[node.id] ?? []}
            temperatureSnapshots={temperatureSnapshots[node.id] ?? []}
            checkedAt={live.checkedAt}
            onElevate={() => onElevate(node)}
            elevationAvailable={targets.some(
              (target) =>
                target.node_id === node.id && target.elevation_available,
            )}
          />
        ))}
      </section>
    </div>
  );
}
