// Page reads are separate from Agent sampling and independent website probes.
const seconds: Record<string, number | null> = {
  overview: 10,
  users: 180,
  storage: 60,
  network: 60,
  operations: null,
  security: 3600,
  permissions: 300,
  "platform-health": 15,
  software: 3600,
  "global-network": 300,
  gitea: 300,
  "ai-tools": 300,
  "programming-environment": 43_200,
  "gpu-comparison": null,
  "agent-management": 30,
};

export function pageRefreshSeconds(page: string): number | null {
  return seconds[page] ?? null;
}

export function pageRefreshLabel(page: string): string | null {
  if (page === "operations") return null;
  if (page === "gpu-comparison") return "文档按需读取";
  if (page === "overview") return "总览实时指标每 1 秒刷新";
  if (page === "platform-health") return "健康 / 网络每 15 秒 · 统计每 1 分钟";
  const interval = pageRefreshSeconds(page);
  if (interval == null) return null;
  const duration = interval % 3600 === 0 ? `${interval / 3600} 小时`
    : interval % 60 === 0 ? `${interval / 60} 分钟` : `${interval} 秒`;
  return `${page === "global-network" ? "代理监控" : ""}每 ${duration}刷新`;
}
