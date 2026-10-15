import type { CSSProperties } from "react";
import { storageUsageAccent } from "../lib/storageUsage";

interface MeterProps {
  label: string;
  value: number | null;
  detail: string;
  tone?: "cyan" | "violet" | "amber" | "danger" | "storage";
  warning?: string;
}

export function Meter({ label, value, detail, tone = "cyan", warning }: MeterProps) {
  const known = value !== null && Number.isFinite(value);
  const safeValue = Math.min(
    100,
    Math.max(0, known ? value : 0),
  );
  const displayValue = known ? `${safeValue.toFixed(1)}%` : "—";
  const accent = tone === "storage" ? storageUsageAccent(known ? safeValue : null) : null;
  return (
    <div className={`meter${tone === "storage" ? " meter--storage" : ""}${warning ? " meter--warning" : ""}`}
      style={accent ? { "--meter-accent": accent } as CSSProperties : undefined}>
      <div className="meter__heading">
        <span>{label}</span>
        <strong>{displayValue}{warning && <span className="meter__warning" role="img" aria-label={warning} title={warning}>⚠︎</span>}</strong>
      </div>
      <div
        className="meter__track"
        aria-label={`${label} ${known ? displayValue : "未采集"}${warning ? `，${warning}` : ""}`}
      >
        <span
          className={`meter__fill meter__fill--${tone}`}
          style={{ width: `${safeValue}%` }}
        />
      </div>
      <span className="meter__detail">{detail}</span>
    </div>
  );
}
