import assert from "node:assert/strict";
import { test } from "node:test";
import { setAllPartitionDetails } from "../src/lib/storageDisclosure.ts";

test("expand and collapse apply to every selected disk without touching other nodes", () => {
  const selected = [{ open: false }, { open: true }, { open: false }];
  const otherNode = { open: true };
  setAllPartitionDetails(selected, true);
  assert.deepEqual(selected.map(detail => detail.open), [true, true, true]);
  setAllPartitionDetails(selected, false);
  assert.deepEqual(selected.map(detail => detail.open), [false, false, false]);
  assert.equal(otherNode.open, true);
});
