import type { HostTemperatures, MetricHistoryPoint, MetricsSnapshot, NodeSummary } from "../types/monitoring.ts";

const SNAPSHOT_WINDOW_MS = 10 * 60_000;
const MAX_SNAPSHOTS_PER_NODE = 128;

// The controller may still serve an older history schema. Retain real full
// snapshots already fetched for the overview; never invent/interpolate readings.
export function recordTemperatureSnapshots(
  previous: Record<string, MetricsSnapshot[]>, nodes: NodeSummary[], now: number,
): Record<string, MetricsSnapshot[]> {
  const next: Record<string, MetricsSnapshot[]> = {};
  for (const node of nodes) {
    const recent = (previous[node.id] ?? []).filter(sample => {
      const gpuAt = Date.parse(sample.gpu_collected_at ?? "");
      const thermalAt = Date.parse(sample.temperatures?.collected_at ?? "");
      return (Number.isFinite(gpuAt) && gpuAt >= now - SNAPSHOT_WINDOW_MS && gpuAt <= now)
        || (Number.isFinite(thermalAt) && thermalAt >= now - SNAPSHOT_WINDOW_MS && thermalAt <= now);
    });
    const metrics = node.online ? node.metrics : null;
    if (metrics) {
      const gpuAt = Date.parse(metrics.gpu_collected_at ?? "");
      const thermalAt = Date.parse(metrics.temperatures?.collected_at ?? "");
      const hasGpu = Number.isFinite(gpuAt) && gpuAt >= now - SNAPSHOT_WINDOW_MS && gpuAt <= now
        && hottest(metrics.gpus.map(gpu => gpu.temperature_celsius)) !== null;
      const hasThermal = Number.isFinite(thermalAt) && thermalAt >= now - SNAPSHOT_WINDOW_MS && thermalAt <= now
        && (validTemperature(metrics.temperatures?.cpu_celsius) || ssdTemperature(metrics.temperatures) !== null);
      if (hasGpu || hasThermal) {
        const last = recent.at(-1);
        if (last?.gpu_collected_at !== metrics.gpu_collected_at
          || last?.temperatures?.collected_at !== metrics.temperatures?.collected_at)
          recent.push(metrics);
      }
    }
    if (recent.length) next[node.id] = recent.slice(-MAX_SNAPSHOTS_PER_NODE);
  }
  return next;
}

export function validTemperature(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 150;
}

export const HOT_TEMPERATURE_C = 70;

export function isHotTemperature(value: unknown): boolean {
  return validTemperature(value) && value > HOT_TEMPERATURE_C;
}

export function hottest(values: (number | null | undefined)[]): number | null {
  const valid = values.filter(validTemperature);
  return valid.length ? Math.max(...valid) : null;
}

export function temperaturePoints(history: (MetricHistoryPoint | MetricsSnapshot)[], metrics: MetricsSnapshot, kind: "cpu" | "gpu" | "ssd") {
  const samples = [...history, metrics];
  const points = new Map<string, { at: string; value: number }>();
  for (const sample of samples) {
    const at = kind === "gpu" ? sample.gpu_collected_at ?? sample.collected_at : sample.temperatures?.collected_at;
    const value = kind === "gpu" ? hottest(sample.gpus.map(gpu => gpu.temperature_celsius))
      : kind === "cpu" ? sample.temperatures?.cpu_celsius : ssdTemperature(sample.temperatures);
    if (at && Number.isFinite(Date.parse(at)) && validTemperature(value)) points.set(at, { at, value });
  }
  return [...points.values()].sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
}

export function ssdTemperature(sample?: HostTemperatures | null): number | null {
  return hottest(sample?.ssds.map(ssd => ssd.temperature_celsius) ?? []);
}

export function formatTemperature(value: unknown): string {
  return validTemperature(value) ? `${value.toFixed(1)}°C` : "温度 —";
}

// Scale only actual readings in the visible window, with 2 C padding and a
// minimum 10 C span. Rounded steps reduce axis jitter for nearly steady sensors.
export function temperatureRange(points: { at: string; value: number }[], start: number, end: number): { min: number; max: number } | null {
  const values = points.filter(point => {
    const time = Date.parse(point.at);
    return Number.isFinite(time) && time >= start && time <= end && validTemperature(point.value);
  }).map(point => point.value);
  if (!values.length) return null;
  const low = Math.min(...values);
  const high = Math.max(...values);
  const targetStep = Math.max(10, high - low + 4) / 4;
  const power = 10 ** Math.floor(Math.log10(targetStep));
  const step = ([1, 2, 5, 10].find(factor => factor * power >= targetStep) ?? 10) * power;
  const min = Math.max(0, Math.floor((low - 2) / step) * step);
  const max = Math.max(min + 10, Math.ceil((high + 2) / step) * step);
  return { min, max };
}
