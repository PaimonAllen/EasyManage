import assert from "node:assert/strict";
import { test } from "node:test";
import { formatTemperature, hottest, isHotTemperature, recordTemperatureSnapshots, temperaturePoints, temperatureRange } from "../src/lib/temperature.ts";
import type { MetricHistoryPoint, MetricsSnapshot, NodeSummary } from "../src/types/monitoring.ts";

test("missing/invalid temperature is not idle; hottest valid device wins", () => {
  assert.equal(hottest([null, undefined, NaN, Infinity, -273, 151]), null);
  assert.equal(hottest([38, 42, null]), 42);
  assert.equal(hottest([0]), 0);
  assert.equal(formatTemperature(null), "温度 —");
  assert.equal(formatTemperature(42.25), "42.3°C");
  assert.equal(isHotTemperature(70), false);
  assert.equal(isHotTemperature(70.01), true);
  assert.equal(isHotTemperature(null), false);
  assert.equal(isHotTemperature(151), false);
});

test("cached temperatures use real sample times, not heartbeat/live timestamps", () => {
  const t = "2026-09-29T08:00:00Z";
  const sample = {
    collected_at: "2026-09-29T08:00:10Z", gpu_collected_at: t,
    temperatures: { collected_at: t, cpu_celsius: 50, ssds: [{ device: "nvme0", temperature_celsius: 42 }] },
    gpus: [{ temperature_celsius: 60 }, { temperature_celsius: 70 }],
  } as MetricsSnapshot;
  const history = [{ ...sample, collected_at: "2026-09-29T08:00:05Z" }] as MetricHistoryPoint[];
  assert.deepEqual(temperaturePoints(history, sample, "cpu"), [{ at: t, value: 50 }]);
  assert.deepEqual(temperaturePoints(history, sample, "ssd"), [{ at: t, value: 42 }]);
  assert.deepEqual(temperaturePoints(history, sample, "gpu"), [{ at: t, value: 70 }]);
  const legacy = { collected_at: sample.collected_at, gpus: [] } as unknown as MetricsSnapshot;
  assert.deepEqual(temperaturePoints([], legacy, "cpu"), []);
  assert.deepEqual(temperaturePoints([], legacy, "ssd"), []);
  assert.deepEqual(temperaturePoints([], legacy, "gpu"), []);
});

test("temperature axis stays at 30–95 degrees, with missing and out-of-window data left absent", () => {
  const start = Date.parse("2026-09-29T08:00:00Z");
  const end = start + 600_000;
  const point = (value: number, time = start + 1000) => ({ at: new Date(time).toISOString(), value });
  assert.deepEqual(temperatureRange([point(60)], start, end), { min: 30, max: 95 });
  assert.deepEqual(temperatureRange([point(42)], start, end), { min: 30, max: 95 });
  assert.deepEqual(temperatureRange([point(0)], start, end), { min: 30, max: 95 });
  assert.equal(temperatureRange([], start, end), null);
  assert.equal(temperatureRange([point(NaN), point(151), point(50, end + 1)], start, end), null);
  const visible = [point(51), point(58)];
  assert.deepEqual(temperatureRange([...visible, point(120, start - 1)], start, end), temperatureRange(visible, start, end));
  const range = temperatureRange([point(5), point(149.9)], start, end)!;
  assert.deepEqual(range,{min:30,max:95});
});

test("existing overview snapshots form real GPU history while legacy API omits temperatures", () => {
  const now = Date.parse("2026-09-30T08:30:00Z");
  const sample = (secondsAgo: number, temperature: number) => ({
    collected_at: new Date(now - secondsAgo * 1000).toISOString(),
    gpu_collected_at: new Date(now - secondsAgo * 1000).toISOString(),
    gpus: [{ temperature_celsius: temperature }],
  }) as MetricsSnapshot;
  const node = (metrics: MetricsSnapshot, online = true) => ({ id: "fixture", online, metrics }) as NodeSummary;
  let saved = recordTemperatureSnapshots({}, [node(sample(20, 59))], now);
  assert.equal(saved.fixture.length, 1);
  saved = recordTemperatureSnapshots(saved, [node(sample(20, 59))], now);
  assert.equal(saved.fixture.length, 1, "same sensor timestamp must not become a new reading");
  saved = recordTemperatureSnapshots(saved, [node(sample(10, 60))], now);
  const points = temperaturePoints(saved.fixture, sample(10, 60), "gpu");
  assert.deepEqual(points.map(point => point.value), [59, 60]);
  assert.equal(temperaturePoints(saved.fixture, sample(10, 60), "cpu").length, 0);
  saved = recordTemperatureSnapshots(saved, [node(sample(1, 61), false)], now);
  assert.equal(saved.fixture.length, 2, "offline snapshot must not add a fake sample");
  saved = recordTemperatureSnapshots(saved, [node(sample(1, 61), false)], now + 601_000);
  assert.equal(saved.fixture, undefined, "past ten minutes are pruned");
});
