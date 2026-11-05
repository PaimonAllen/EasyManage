import { useEffect, useState } from "react";

// One request at a time, only while visible; every request has a deadline.
export function usePollingResource<T>(load: (signal: AbortSignal) => Promise<T>, intervalMs: number, refreshKey = 0) {
  const [data, setData] = useState<T>();
  const [error, setError] = useState("");
  useEffect(() => {
    const controller = new AbortController();
    let pending = false;
    async function refresh() {
      if (pending || document.hidden || controller.signal.aborted) return;
      pending = true;
      try {
        const value = await load(AbortSignal.any([controller.signal, AbortSignal.timeout(10_000)]));
        if (!controller.signal.aborted) { setData(value); setError(""); }
      } catch (caught) {
        if (!controller.signal.aborted) setError(caught instanceof Error ? caught.message : "平台资源读取失败");
      } finally { pending = false; }
    }
    void refresh();
    const timer = window.setInterval(() => void refresh(), intervalMs);
    document.addEventListener("visibilitychange", refresh);
    return () => { controller.abort(); window.clearInterval(timer); document.removeEventListener("visibilitychange", refresh); };
  }, [load, intervalMs, refreshKey]);
  return { data, error };
}
