import { useId, type ReactNode } from "react";
import { formatTemperature, HOT_TEMPERATURE_C, isHotTemperature, temperatureRange } from "../lib/temperature";

export interface ChartPoint {
  at: string;
  value: number;
}

export interface ChartSeries {
  label: string;
  color: string;
  points: ChartPoint[];
}

interface HistoryChartProps {
  temperature?: { points: ChartPoint[]; value: number | null; detail: string; gapSeconds: number };
  title: string;
  headline: ReactNode;
  detail: string;
  series: ChartSeries[];
  maxValue: number;
  maxLabel: string;
  windowEnd?: string;
  windowSeconds?: number;
  gapSeconds?: number;
  emptyLabel?: string;
}

const WIDTH = 600;
const HEIGHT = 160;
const DEFAULT_GAP_MS = 35_000;
const clockFormatter = new Intl.DateTimeFormat("zh-CN", { hour: "2-digit", minute: "2-digit", hourCycle: "h23" });

function segmentPoints(points: ChartPoint[], start: number, end: number, maxValue: number, gapMs: number, minValue = 0) {
  const valid = points.map((point) => ({ time: Date.parse(point.at), value: point.value }))
    .filter((point) => Number.isFinite(point.time) && Number.isFinite(point.value) && point.time >= start && point.time <= end)
    .sort((a, b) => a.time - b.time);
  const segments: { x: number; y: number }[][] = [];
  let previous = -Infinity;
  for (const point of valid) {
    if (point.time - previous > gapMs) segments.push([]);
    segments[segments.length - 1].push({
      x: ((point.time - start) / (end - start)) * WIDTH,
      y: HEIGHT - ((Math.min(maxValue, Math.max(minValue, point.value)) - minValue) / (maxValue - minValue)) * HEIGHT,
    });
    previous = point.time;
  }
  return segments;
}

function line(points: { x: number; y: number }[]) {
  return points.map((point, index) => `${index === 0 ? "M" : "L"}${point.x.toFixed(1)},${point.y.toFixed(1)}`).join(" ");
}

export function HistoryChart({ title, headline, detail, series, maxValue, maxLabel, temperature, windowEnd, windowSeconds = 600, gapSeconds = DEFAULT_GAP_MS / 1_000, emptyLabel = "等待历史采样" }: HistoryChartProps) {
  const hotClipId = useId().replace(/[^a-zA-Z0-9_-]/g, "");
  const end = Date.parse(windowEnd ?? "") || Date.now();
  const start = end - windowSeconds * 1_000;
  const scale = Number.isFinite(maxValue) && maxValue > 0 ? maxValue : 1;
  const rendered = series.map((item) => ({ ...item, segments: segmentPoints(item.points, start, end, scale, gapSeconds * 1_000) }));
  const thermalScale = temperature ? temperatureRange(temperature.points, start, end) : null;
  const thermalSegments = temperature && thermalScale ? segmentPoints(temperature.points, start, end, thermalScale.max, temperature.gapSeconds * 1000, thermalScale.min) : [];
  const hotThresholdY = thermalScale ? HEIGHT - ((HOT_TEMPERATURE_C - thermalScale.min) / (thermalScale.max - thermalScale.min)) * HEIGHT : null;
  const hasHotSegment = hotThresholdY !== null && thermalSegments.some(segment => segment.some(point => point.y < hotThresholdY));
  const thermalSampleCount = thermalSegments.reduce((count, segment) => count + segment.length, 0);
  const sampleCount = Math.max(0, ...rendered.map((item) => item.segments.reduce((total, segment) => total + segment.length, 0)));
  return <div className="history-chart">
    <div className="history-chart__heading">
      <div className="history-chart__heading-meta">
        <span title={detail}>{title}</span>
        <small title={detail}>{detail}</small>
      </div>
      <strong className="history-chart__headline">
        {headline}
        {temperature && <span className={`history-chart__temperature${isHotTemperature(temperature.value) ? " history-chart__temperature--hot" : ""}`} title={temperature.detail}>{formatTemperature(temperature.value)}</span>}
      </strong>
    </div>
    <div className="history-chart__plot">
      <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} preserveAspectRatio="none" role="img" aria-label={`${title}近 ${Math.round(windowSeconds / 60)} 分钟真实采样曲线${temperature ? "，右轴温度为动态范围，超过 70°C 的线段为红色" : ""}`}>
        <title>{`${title} · 近 ${Math.round(windowSeconds / 60)} 分钟${temperature ? " · 右轴温度（°C），超过 70°C 为红色" : ""}`}</title>
        {hasHotSegment && <defs><clipPath id={hotClipId} clipPathUnits="userSpaceOnUse"><rect x="0" y="0" width={WIDTH} height={Math.min(HEIGHT, Math.max(0, hotThresholdY ?? 0))} /></clipPath></defs>}
        {Array.from({ length: 13 }, (_, index) => <line className="history-chart__grid" key={`v${index}`} x1={index * WIDTH / 12} y1="0" x2={index * WIDTH / 12} y2={HEIGHT} />)}
        {Array.from({ length: 5 }, (_, index) => <line className="history-chart__grid" key={`h${index}`} x1="0" y1={index * HEIGHT / 4} x2={WIDTH} y2={index * HEIGHT / 4} />)}
        {rendered.map((item) => item.segments.map((segment, index) => <g key={`${item.label}-${index}`}>
          {rendered.length === 1 && segment.length > 1 && <path d={`${line(segment)} L${segment[segment.length - 1].x.toFixed(1)},${HEIGHT} L${segment[0].x.toFixed(1)},${HEIGHT} Z`} fill={item.color} opacity="0.12" />}
          <path d={line(segment)} fill="none" stroke={item.color} strokeWidth="2.5" vectorEffect="non-scaling-stroke" strokeLinejoin="round" strokeLinecap="round" />
          {segment.length === 1 && <circle cx={segment[0].x} cy={segment[0].y} r="3" fill={item.color} vectorEffect="non-scaling-stroke" />}
        </g>))}
        {thermalSegments.map((segment, index) => <g key={`temperature-${index}`}>
          <path d={line(segment)} fill="none" stroke="var(--chart-blue)" strokeWidth="2.5" vectorEffect="non-scaling-stroke" strokeLinejoin="round" strokeLinecap="round" />
          {hasHotSegment && segment.length > 1 && <path d={line(segment)} fill="none" stroke="var(--chart-hot)" strokeWidth="2.5" vectorEffect="non-scaling-stroke" strokeLinejoin="round" strokeLinecap="round" clipPath={`url(#${hotClipId})`} />}
          {segment.length === 1 && <circle cx={Math.max(8, Math.min(WIDTH - 8, segment[0].x))} cy={segment[0].y} r="8" fill={hotThresholdY !== null && segment[0].y < hotThresholdY ? "var(--chart-hot)" : "var(--chart-blue)"} />}
        </g>)}
      </svg>
      <span className={`history-chart__max${temperature ? " history-chart__max--primary" : ""}`}>{maxLabel}</span>
      {temperature && <span className={`history-chart__thermal-max${isHotTemperature(thermalScale?.max) ? " history-chart__thermal-max--hot" : ""}`}>{thermalScale ? `${thermalScale.max}°C` : "—"}</span>}
      <span className={`history-chart__zero${temperature ? " history-chart__zero--primary" : ""}`}>0</span>
      {temperature && <span className={`history-chart__thermal-min${isHotTemperature(thermalScale?.min) ? " history-chart__thermal-min--hot" : ""}`}>{thermalScale ? `${thermalScale.min}°C` : "—"}</span>}
      {sampleCount === 0 && thermalSampleCount === 0 && <div className="history-chart__empty">{emptyLabel}</div>}
    </div>
    <div className="history-chart__footer">
      <time dateTime={new Date(start).toISOString()}>{clockFormatter.format(start)}</time>
      <div className="history-chart__legend">{rendered.map((item) => <span key={item.label}><i style={{ background: item.color }} />{item.label}</span>)}{temperature && <span title={temperature.detail}><i style={{ background: "var(--chart-blue)" }} />{title === "磁盘 IO" ? "SSD 温度" : "温度"}</span>}</div>
      <time dateTime={new Date(end).toISOString()}>{clockFormatter.format(end)}</time>
    </div>
  </div>;
}
