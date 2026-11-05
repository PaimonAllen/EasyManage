import { cloneElement, useEffect, useId, useState, type ReactElement, type ReactNode } from "react";
import type { NodeSummary } from "../types/monitoring";

// Shared by monitoring pages; management controls stay in the visible header.
export function NodePanel({ node, header, children, notice, className = "data-panel" }: {
  node: NodeSummary;
  header: ReactElement<{ children?: ReactNode }> | ((expanded: boolean) => ReactElement<{ children?: ReactNode }>);
  children: () => ReactNode;
  notice?: ReactNode;
  className?: string;
}) {
  const bodyId = useId();
  const paused = Boolean(node.monitoring_paused||node.agent_service_desired_stopped||node.agent_service_state&&node.agent_service_state!=="running");
  const pauseKey = `${node.id}:${node.monitoring_revision ?? 0}:${node.agent_service_revision??0}`;
  const [openedPause, setOpenedPause] = useState<string | null>(null);
  // A normal data refresh preserves a manual expansion. A new pause revision
  // defaults closed; resuming also clears the choice for legacy snapshots.
  useEffect(() => { if (!paused) setOpenedPause(null); }, [paused, node.id]);
  const expanded = !paused || openedPause === pauseKey;
  const heading = typeof header === "function" ? header(expanded) : header;
  const toggle = <button type="button" className="button button--secondary button--small node-panel__toggle"
    aria-expanded={expanded} aria-controls={bodyId}
    aria-label={`${expanded ? "收起" : "展开"} ${node.hostname} 的监控详情`}
    onClick={() => setOpenedPause(expanded ? null : pauseKey)}>
    <span aria-hidden="true">{expanded ? "▾" : "▸"}</span>{expanded ? "收起" : "展开"}
  </button>;
  return <article className={`${className}${paused ? " node-panel--paused" : ""}`}>
    {paused ? cloneElement(heading, undefined, heading.props.children, toggle) : heading}
    {notice}
    <div id={bodyId} hidden={!expanded}>{expanded ? children() : null}</div>
  </article>;
}
