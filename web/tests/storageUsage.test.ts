import assert from "node:assert/strict";
import { test } from "node:test";
import { storageUsageAccent, storageUsageOver90, storageUsagePercent } from "../src/lib/storageUsage.ts";

test("mounted filesystem warning is strictly above 90%", () => {
  assert.equal(storageUsageOver90(1000, 100), false);
  assert.equal(storageUsageOver90(1000, 99), true);
  assert.equal(storageUsageOver90(1000, 0), true);
  assert.equal(storageUsageOver90(1000, 101), false);
  assert.equal(storageUsagePercent(1000, 100), 90);
  assert.equal(storageUsagePercent(100, 81), 19);
});

test("missing or inconsistent capacity cannot raise a false warning", () => {
  for (const [total, available] of [[0, 0], [-1, 0], [100, -1], [100, 101], [NaN, 0], [100, Infinity]]) {
    assert.equal(storageUsageOver90(total, available), false);
    assert.equal(storageUsagePercent(total, available), null);
  }
});

test("storage accent follows green below 20%, progressively deeper orange through 90%, then red", () => {
  assert.equal(storageUsageAccent(19.9), "var(--storage-green)");
  assert.equal(storageUsageAccent(20), "color-mix(in srgb, var(--storage-orange-light) 100.0%, var(--storage-orange-deep) 0.0%)");
  assert.equal(storageUsageAccent(55), "color-mix(in srgb, var(--storage-orange-light) 50.0%, var(--storage-orange-deep) 50.0%)");
  assert.equal(storageUsageAccent(90), "color-mix(in srgb, var(--storage-orange-light) 0.0%, var(--storage-orange-deep) 100.0%)");
  assert.equal(storageUsageAccent(90.01), "var(--chart-hot)");
  assert.equal(storageUsageAccent(null), null);
});
