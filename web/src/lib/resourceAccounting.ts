export type ResourceEvidence = "current" | "missing" | "paused" | "offline" | "stale" | "unknown";

export const RESOURCE_EVIDENCE: Record<ResourceEvidence, string> = {
  current: "当前采样",
  missing: "尚无采样",
  paused: "监控已暂停 · 历史采样",
  offline: "节点离线 · 历史采样",
  stale: "采样已过期 · 历史值",
  unknown: "采样时间未知或异常",
};

// Freshness uses the real sampling timestamp, not heartbeat/HTTP fetch time.
export function resourceEvidence({ present, collectedAt, online = true, paused = false, seconds = 15, now = Date.now() }: {
  present: boolean;
  collectedAt?: string | null;
  online?: boolean;
  paused?: boolean;
  seconds?: number;
  now?: number;
}): ResourceEvidence {
  if (!present) return "missing";
  if (paused) return "paused";
  if (!online) return "offline";
  const at = collectedAt ? Date.parse(collectedAt) : NaN;
  if (!Number.isFinite(at) || !Number.isFinite(now) || at > now + 30_000) return "unknown";
  const cadence = Number.isFinite(seconds) && seconds > 0 ? seconds : 15;
  return now - at > Math.max(cadence * 2, cadence + 30) * 1000 ? "stale" : "current";
}
