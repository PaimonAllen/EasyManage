const byteUnits = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

function byteUnit(value: number): number {
  return Math.max(0, Math.min(Math.floor(Math.log(value) / Math.log(1024)), byteUnits.length - 1));
}

export function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value < 0) return "—";
  if (value === 0) return "0 B";
  const unit = byteUnit(value);
  const amount = value < 1 ? Number(value.toPrecision(3)).toString() : (value / 1024 ** unit).toFixed(unit > 2 ? 1 : 0);
  return `${amount} ${byteUnits[unit]}`;
}

export function formatRate(value: number): string {
  if (!Number.isFinite(value) || value < 0) return "—";
  if (value === 0) return "0 B/s";
  const unit = byteUnit(value);
  // Significant digits retain tiny, nonzero rates instead of rounding to zero.
  const amount = Number((value / 1024 ** unit).toPrecision(3)).toString();
  return `${amount} ${byteUnits[unit]}/s`;
}
export const percent = (used: number, total: number) =>
  total > 0 ? (used / total) * 100 : 0;
export const formatDate = (value?: string | null) =>
  value ? new Date(value).toLocaleString("zh-CN", { hour12: false }) : "—";

export function formatUptime(seconds: number): string {
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3_600);
  return days > 0 ? `${days} 天 ${hours} 小时` : `${hours} 小时`;
}
