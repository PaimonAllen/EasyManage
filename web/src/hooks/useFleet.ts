import { useCallback, useEffect, useRef, useState } from "react";
import { fetchNodes } from "../api/client";
import type { NodeListResponse } from "../types/monitoring";

export function useFleet(refreshIntervalMs = 5_000, enabled = true, includeSoftware = false) {
  const [data, setData] = useState<NodeListResponse | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const request = useRef<AbortController | null>(null);

  const refresh = useCallback(async () => {
    if (request.current) return;
    const controller = new AbortController();
    request.current = controller;
    try {
      setData(await fetchNodes(controller.signal, includeSoftware));
      setError("");
    } catch (caught) {
      if (!controller.signal.aborted)
        setError(caught instanceof Error ? caught.message : "无法读取平台数据");
    } finally {
      if (!controller.signal.aborted) setLoading(false);
      if (request.current === controller) request.current = null;
    }
  }, [includeSoftware]);

  useEffect(() => {
    if (!enabled) return;
    const refreshIfVisible = () => {
      if (!document.hidden) void refresh();
    };
    refreshIfVisible();
    const timer = window.setInterval(refreshIfVisible, refreshIntervalMs);
    document.addEventListener("visibilitychange", refreshIfVisible);
    return () => {
      document.removeEventListener("visibilitychange", refreshIfVisible);
      request.current?.abort();
      request.current = null;
      window.clearInterval(timer);
    };
  }, [refresh, refreshIntervalMs, enabled]);

  return { data, error, loading, refresh };
}
