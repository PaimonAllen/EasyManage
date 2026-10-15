import assert from "node:assert/strict";
import { test } from "node:test";
import { diskRecording } from "../src/lib/diskRecording.ts";

const disk = (model: string | null, rotational: boolean | null = true, kind = "disk") => ({ model, rotational, kind });

test("manufacturer-confirmed models match exact base number and observed hardware suffixes", () => {
  assert.equal(diskRecording(disk("ST2000DM005-2CW102"))?.kind, "smr");
  assert.equal(diskRecording(disk("ST4000DM004-2U91"))?.kind, "smr");
  assert.equal(diskRecording(disk("ST4000NM0035-1V4107"))?.kind, "cmr");
  assert.equal(diskRecording(disk(" seagate st2000dm005-2cw102 "))?.matchedModel, "ST2000DM005");
  const result = diskRecording(disk("ST4000NM0035"))!;
  assert.equal(result.verifiedAt, "2026-09-29");
  assert.ok(result.sources.every(source => source.url.startsWith("https://www.seagate.com/")));
});

test("do not infer recording technology from family, prefixes, capacity or missing model", () => {
  for (const model of ["ST2000DM006", "ST2000DM0050", "ST4000DM000", "ST4000NM0035X", "bridge-ST2000DM005", "ST2000DM005/foo", null]) {
    assert.equal(diskRecording(disk(model))?.kind, "unknown");
    assert.deepEqual(diskRecording(disk(model))?.sources, []);
  }
  assert.equal(diskRecording(disk("ST2000DM005", null))?.kind, "smr");
});

test("SSD, partitions and virtual aggregate devices are not marked CMR or SMR", () => {
  for (const model of ["ZHITAI TiPlus7100 2TB", "Samsung SSD 990 EVO Plus 1TB", "Predator SSD GM7 1TB"]) {
    assert.equal(diskRecording(disk(model, false)), null);
  }
  for (const kind of ["part", "lvm", "raid1", "loop"]) {
    assert.equal(diskRecording(disk("ST2000DM005", true, kind)), null);
  }
});
