// Only run in the documented isolated environment.
import assert from "node:assert/strict";
import { test } from "node:test";
import { inventoryFresh, permissionFreshSeconds, policyApplied } from "../src/lib/collection.ts";
import type { CollectionPolicy, NodeSummary } from "../src/types/monitoring.ts";

function node(ageSeconds: number, extendedSeconds?: number): NodeSummary {
  return { online: true, metrics: { inventory_collected_at: new Date(Date.now() - ageSeconds * 1000).toISOString(), collection_policy: extendedSeconds == null ? null : { extended_seconds: extendedSeconds, permission_seconds: 600, report_seconds: 10 } } } as NodeSummary;
}
test("freshness follows applied sampling intervals with legacy fallbacks", () => {
  assert.equal(inventoryFresh(node(120)), true);
  assert.equal(inventoryFresh(node(200)), false);
  assert.equal(inventoryFresh(node(200, 300)), true);
  assert.equal(inventoryFresh(node(700, 300)), false);
  assert.equal(inventoryFresh({ ...node(1, 300), online: false }), false);
});
test("permission review age follows the configured audit interval", () => {
  assert.equal(permissionFreshSeconds(node(1)), 480);
  assert.equal(permissionFreshSeconds(node(1, 300)), 730);
});
test("application confirmation checks the actual intervals, not just a revision number", () => {
  const expected = { revision: 3, live_seconds: 2, gpu_seconds: 30 } as CollectionPolicy;
  assert.equal(policyApplied(null, expected), false);
  assert.equal(policyApplied({ ...expected }, expected), true);
  assert.equal(policyApplied({ ...expected, live_seconds: 1 }, expected), false);
  assert.equal(policyApplied({ ...expected, revision: 2 }, expected), false);
});
