import { useEffect, useState } from "react";
import { fetchNodeHistory } from "../api/client";
import type { NodeMetricHistory } from "../types/monitoring";

export function useMetricHistory(nodeIds: string[]) {
  const [history, setHistory] = useState<Record<string, NodeMetricHistory>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const ids = nodeIds.join(",");

  useEffect(() => {
    const controller = new AbortController();
    const requestedIds = ids ? ids.split(",") : [];
    let pending = false;
    async function refresh() {
      if (pending || document.hidden || requestedIds.length === 0) return;
      pending = true;
      try {
        const results = await Promise.allSettled(
          requestedIds.map((nodeId) => fetchNodeHistory(nodeId, controller.signal)),
        );
        if (controller.signal.aborted) return;
        setHistory((previous) => {
          const next: Record<string, NodeMetricHistory> = {};
          results.forEach((result, index) => {
            const nodeId = requestedIds[index];
            if (result.status === "fulfilled") next[nodeId] = result.value;
            else if (previous[nodeId]) next[nodeId] = previous[nodeId];
          });
          return next;
        });
        setErrors(Object.fromEntries(results.flatMap((result, index) =>
          result.status === "rejected" ? [[requestedIds[index], "历史曲线暂不可用，显示上次成功的数据"]] : [],
        )));
      } finally {
        pending = false;
      }
    }
    void refresh();
    const timer = window.setInterval(() => void refresh(), 30_000);
    const onVisibility = () => { if (!document.hidden) void refresh(); };
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      document.removeEventListener("visibilitychange", onVisibility);
      controller.abort();
      window.clearInterval(timer);
    };
  }, [ids]);

  return { history, errors };
}
