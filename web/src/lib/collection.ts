import type { CollectionPolicy, NodeSummary } from "../types/monitoring.ts";

export function policyApplied(applied: CollectionPolicy | null, expected: CollectionPolicy): boolean {
  return !!applied && Object.entries(expected).every(([key, value]) => applied[key as keyof CollectionPolicy] === value);
}

export type InventoryCategory = "users" | "software" | "proxy" | "ai_tools" | "programming";

export function inventorySample(node: NodeSummary, category?: InventoryCategory) {
  if (category && node.metrics?.inventory_samples) {
    const sample = node.metrics.inventory_samples[category];
    return { timestamp: sample?.collected_at, seconds: sample?.interval_seconds ?? 60 };
  }
  return { timestamp: node.metrics?.inventory_collected_at, seconds: node.metrics?.collection_policy?.extended_seconds ?? 60 };
}

export function inventoryFresh(node: NodeSummary, category?: InventoryCategory): boolean {
  const sample = inventorySample(node, category);
  const at = Date.parse(sample.timestamp ?? "");
  const seconds = sample.seconds * 2 + 30;
  return node.online && Number.isFinite(at) && Date.now() - at <= seconds * 1000;
}

export function permissionFreshSeconds(node: NodeSummary): number {
  const policy = node.metrics?.collection_policy;
  return Math.max(480, (policy?.permission_seconds ?? 300) + (policy?.report_seconds ?? 10) + 120);
}

export function securityFresh(node: NodeSummary): boolean {
  const security = node.metrics?.host_security;
  if (!security?.collected_at && security?.interval_seconds == null) return inventoryFresh(node);
  const at = Date.parse(security?.collected_at ?? "");
  const seconds = security?.interval_seconds;
  const age = Date.now() - at;
  return node.online && Number.isFinite(at) && seconds === 3600 && age >= 0 &&
    age <= (seconds + (node.metrics?.collection_policy?.report_seconds ?? 10) + 120) * 1000;
}
