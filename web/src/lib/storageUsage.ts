/** Ignore missing or inconsistent capacity instead of painting it as healthy. */
export function storageUsagePercent(totalBytes: number, availableBytes: number): number | null {
  if (!Number.isFinite(totalBytes) || !Number.isFinite(availableBytes)
    || totalBytes <= 0 || availableBytes < 0 || availableBytes > totalBytes) return null;
  return ((totalBytes - availableBytes) / totalBytes) * 100;
}

export function storageUsageOver90(totalBytes: number, availableBytes: number): boolean {
  const usage = storageUsagePercent(totalBytes, availableBytes);
  return usage !== null && usage > 90;
}

/** Theme-aware green / pale-to-deep orange / critical red accent. */
export function storageUsageAccent(usage: number | null): string | null {
  if (usage === null || !Number.isFinite(usage) || usage < 0 || usage > 100) return null;
  if (usage < 20) return "var(--storage-green)";
  if (usage > 90) return "var(--chart-hot)";
  const deep = ((usage - 20) / 70) * 100;
  return `color-mix(in srgb, var(--storage-orange-light) ${(100 - deep).toFixed(1)}%, var(--storage-orange-deep) ${deep.toFixed(1)}%)`;
}
