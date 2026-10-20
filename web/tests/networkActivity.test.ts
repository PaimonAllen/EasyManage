// Only run in the documented private test environment.
import assert from "node:assert/strict";
import { test } from "node:test";
import { networkRates } from "../src/lib/networkActivity.ts";
import type { NetworkActivity,NetworkGroupMetric } from "../src/types/monitoring.ts";

const row:NetworkGroupMetric={group:"agent_live",started:10,completed:10,failed:0,in_flight:0,open_connections:0,received_bytes:100,submitted_bytes:200};
const previous:NetworkActivity={process_id:42,started_at:"2026-01-01T00:00:00Z",collected_at:"2026-01-01T00:01:00Z",groups:[row]};
const nextRow={...row,started:20,received_bytes:120,submitted_bytes:250};
const next={...previous,collected_at:"2026-01-01T00:01:10Z",groups:[nextRow]};
test("rates use two real samples, including true zero and fractional byte rates",()=>{
  assert.deepEqual(networkRates(next,previous,nextRow),{received:2,submitted:5,requests:1});
  assert.deepEqual(networkRates({...next,groups:[row]},previous,row),{received:0,submitted:0,requests:0});
  assert.equal(networkRates(next,previous,{...nextRow,received_bytes:101})?.received,.1);
});
test("first samples, restarts, stale samples and counter rollback are unknown, not zero",()=>{
  assert.equal(networkRates(next,null,nextRow),null);
  assert.equal(networkRates({...next,process_id:43},previous,nextRow),null);
  assert.equal(networkRates({...next,started_at:"2026-01-01T00:00:01Z"},previous,nextRow),null);
  assert.equal(networkRates(previous,previous,row),null);
  assert.equal(networkRates({...next,collected_at:"invalid"},previous,nextRow),null);
  assert.equal(networkRates(next,previous,{...nextRow,submitted_bytes:100}),null);
  assert.equal(networkRates(next,previous,{...nextRow,started:1}),null);
});
