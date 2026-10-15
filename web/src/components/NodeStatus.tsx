import type { NodeSummary } from "../types/monitoring";
import { formatDate } from "../lib/format";

export function SnapshotAge({
  node,
  inventory = false,
  disk = false,
}: {
  node: NodeSummary;
  inventory?: boolean;
  disk?: boolean;
}) {
  const timestamp = disk
    ? node.metrics?.disk_collected_at ?? node.metrics?.collected_at
    : inventory
    ? node.metrics?.inventory_collected_at
    : node.metrics?.collected_at;
  const collected = timestamp ? Date.parse(timestamp) : NaN;
  if (!Number.isFinite(collected))
    return (
      <span className="snapshot-age">
        尚未收到{disk ? "磁盘" : inventory ? "清单" : "指标"}快照
      </span>
    );
  const age = Math.max(0, Math.floor((Date.now() - collected) / 1_000));
  const policy = node.metrics?.collection_policy;
  const stale = !node.online || age > (disk ? (policy?.disk_seconds ?? 30) * 2 + 30 : inventory ? (policy?.extended_seconds ?? 60) * 2 + 30 : 45);
  const ageLabel =
    age < 60
      ? `${age} 秒前`
      : age < 3600
        ? `${Math.floor(age / 60)} 分钟前`
        : `${Math.floor(age / 3600)} 小时前`;
  return (
    <span
      className={`snapshot-age ${stale ? "snapshot-age--stale" : ""}`}
      title={formatDate(timestamp)}
    >
      {disk ? "磁盘" : inventory ? "清单" : "指标"}采集于 {formatDate(timestamp)} · {ageLabel}
      {stale && " · 历史快照"}
    </span>
  );
}

export function NodeStatus({ node }: { node: NodeSummary }) {
  return (
    <span
      className={`status ${node.online ? "status--online" : "status--offline"}`}
    >
      <i />
      {node.online ? "在线" : "离线"}
    </span>
  );
}

export function PrivilegeBadge({ node }: { node: NodeSummary }) {
  const privilege = node.metrics?.privilege;
  if (!privilege) return <span className="badge badge--muted">权限待上报</span>;
  const full = privilege.mode === "full" && privilege.uid === 0;
  return (
    <span
      className={`badge ${full ? "badge--full" : "badge--basic"}`}
      title={`运行用户 ${privilege.username} · UID ${privilege.uid}`}
    >
      {full ? "Full" : "Basic"} · {privilege.username}
    </span>
  );
}

export function CollectionNotes({
  node,
  components,
}: {
  node: NodeSummary;
  components?: string[];
}) {
  const issues = (node.metrics?.collection_issues ?? []).filter(
    (issue) =>
      !components ||
      components.some((component) => issue.component.includes(component)),
  );
  return issues.length ? (
    <details className="collection-notes">
      <summary>{issues.length} 项采集限制</summary>
      <ul>
        {issues.map((issue, index) => (
          <li key={`${issue.component}-${index}`}>
            <strong>{issue.component}</strong>：{issue.message}
          </li>
        ))}
      </ul>
    </details>
  ) : null;
}
