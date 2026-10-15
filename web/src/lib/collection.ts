import type { CollectionPolicy, NodeSummary } from "../types/monitoring.ts";

export function policyApplied(applied: CollectionPolicy | null, expected: CollectionPolicy): boolean {
  return !!applied && Object.entries(expected).every(([key, value]) => applied[key as keyof CollectionPolicy] === value);
}

export function inventoryFresh(node: NodeSummary): boolean {
  const at = Date.parse(node.metrics?.inventory_collected_at ?? "");
  const seconds = (node.metrics?.collection_policy?.extended_seconds ?? 60) * 2 + 30;
  return node.online && Number.isFinite(at) && Date.now() - at <= seconds * 1000;
}

export function permissionFreshSeconds(node: NodeSummary): number {
  const policy = node.metrics?.collection_policy;
  return Math.max(480, (policy?.permission_seconds ?? 300) + (policy?.report_seconds ?? 10) + 120);
}
