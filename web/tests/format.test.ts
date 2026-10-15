// Run only in the documented isolated virtual environment, not on production.
import assert from "node:assert/strict";
import { test } from "node:test";
import { formatBytes, formatRate } from "../src/lib/format.ts";

test("fractional bytes use the base unit, never a negative unit index", () => {
  assert.equal(formatBytes(0.374), "0.374 B");
  assert.equal(formatRate(0.7534971989370537), "0.753 B/s");
  assert.equal(formatRate(0.0000045), "0.0000045 B/s");
  assert.equal(formatRate(5e-12), "5e-12 B/s");
});

test("rates retain unit precision at each magnitude", () => {
  assert.equal(formatRate(1), "1 B/s");
  assert.equal(formatRate(1638.1029104891547), "1.6 KiB/s");
  assert.equal(formatRate(1536), "1.5 KiB/s");
  assert.equal(formatRate(1048576), "1 MiB/s");
  assert.equal(formatBytes(1048576), "1 MiB");
});

test("invalid or missing inputs are not displayed as zero", () => {
  assert.equal(formatRate(0), "0 B/s");
  for (const value of [NaN, Infinity, -1]) {
    assert.equal(formatRate(value), "—");
    assert.equal(formatBytes(value), "—");
  }
});

test("every positive finite rate remains nonzero with a valid unit", () => {
  for (const value of [Number.MIN_VALUE, 1e-12, 0.001, 0.9999, 1, 1023, 1024, 1e12, Number.MAX_VALUE]) {
    const output = formatRate(value);
    assert.doesNotMatch(output, /undefined|NaN|Infinity/);
    assert.match(output, / (B|KiB|MiB|GiB|TiB|PiB)\/s$/);
    assert.ok(Number.parseFloat(output) > 0, output);
  }
});
