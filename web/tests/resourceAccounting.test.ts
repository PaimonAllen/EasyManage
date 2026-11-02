import assert from "node:assert/strict";
import { test } from "node:test";
import { resourceEvidence } from "../src/lib/resourceAccounting.ts";

test("resource evidence uses sample age and preserves paused/offline historical values", () => {
  const now = Date.parse("2026-11-02T03:00:00Z");
  const sample = { present:true, collectedAt:"2026-11-02T02:59:45Z", now };
  assert.equal(resourceEvidence(sample), "current");
  assert.equal(resourceEvidence({...sample, now:now+60_000}), "stale");
  assert.equal(resourceEvidence({...sample, online:false}), "offline");
  assert.equal(resourceEvidence({...sample, paused:true, online:false}), "paused");
  assert.equal(resourceEvidence({...sample, present:false}), "missing");
  assert.equal(resourceEvidence({...sample, collectedAt:null}), "unknown");
  assert.equal(resourceEvidence({...sample, collectedAt:"invalid"}), "unknown");
  assert.equal(resourceEvidence({...sample, collectedAt:"2026-11-02T03:01:00Z"}), "unknown");
});

test("resource freshness respects configured low sampling frequency", () => {
  const now = Date.parse("2026-11-02T03:00:00Z");
  assert.equal(resourceEvidence({present:true,collectedAt:"2026-11-02T02:55:00Z",seconds:300,now}), "current");
  assert.equal(resourceEvidence({present:true,collectedAt:"2026-11-02T02:45:00Z",seconds:300,now}), "stale");
});
