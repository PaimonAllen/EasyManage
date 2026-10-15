import { useEffect, useState } from "react";
import { fetchLiveNodes } from "../api/client";
import type { LiveNode } from "../types/monitoring";

const LIVE_HISTORY_MS = 5 * 60_000;
const MAX_SAMPLES_PER_NODE = 300;

export function useLiveMetrics() {
  const [samples, setSamples] = useState<Record<string, LiveNode[]>>({});
  const [error, setError] = useState("");
  const [checkedAt, setCheckedAt] = useState(Date.now());

  useEffect(() => {
    const controller = new AbortController();
    let pending = false;
    let serverClockOffset = 0;
    async function refresh() {
      if (pending || document.hidden) return;
      pending = true;
      try {
        const response = await fetchLiveNodes(controller.signal);
        if (controller.signal.aborted) return;
        const serverNow = Date.parse(response.generated_at);
        const now = Number.isFinite(serverNow) ? serverNow : Date.now();
        serverClockOffset = now - Date.now();
        setSamples((previous) => {
          const next: Record<string, LiveNode[]> = {};
          for (const [nodeId, history] of Object.entries(previous)) {
            const recent = history.filter((sample) => Date.parse(sample.received_at) >= now - LIVE_HISTORY_MS);
            if (recent.length > 0) next[nodeId] = recent;
          }
          for (const sample of response.nodes) {
            const history = next[sample.node_id] ?? [];
            if (history.at(-1)?.received_at !== sample.received_at)
              next[sample.node_id] = [...history, sample].slice(-MAX_SAMPLES_PER_NODE);
          }
          return next;
        });
        setError("");
        setCheckedAt(now);
      } catch (caught) {
        if (!controller.signal.aborted) {
          setError(caught instanceof Error ? caught.message : "实时指标读取失败");
          setCheckedAt(Date.now() + serverClockOffset);
        }
      } finally {
        pending = false;
      }
    }
    void refresh();
    const timer = window.setInterval(() => void refresh(), 1_000);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      controller.abort();
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, []);

  return { samples, error, checkedAt };
}
