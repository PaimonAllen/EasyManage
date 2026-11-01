import { useId, type ReactNode } from "react";
import { formatTemperature, HOT_TEMPERATURE_C, isHotTemperature, temperatureRange, TEMPERATURE_AXIS_MIN_C, TEMPERATURE_AXIS_MAX_C, validTemperature } from "../lib/temperature";

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
  detailTitle?: string;
  series: ChartSeries[];
  maxValue: number;
  maxLabel: string;
  windowEnd?: string;
  windowSeconds?: number;
  gapSeconds?: number;
  emptyLabel?: string;
  rangeLabel?: string;
  timeFormatter?: (time:number)=>string;
}

const WIDTH = 600;
const HEIGHT = 160;
const DEFAULT_GAP_MS = 35_000;
const clockFormatter = new Intl.DateTimeFormat("zh-CN", { hour: "2-digit", minute: "2-digit", hourCycle: "h23" });

type PlotPoint = { x:number; y:number; value:number };
function segmentPoints(points: ChartPoint[], start: number, end: number, maxValue: number, gapMs: number, minValue = 0, splitAt:number[] = []) {
  const valid = points.map((point) => ({ time: Date.parse(point.at), value: point.value }))
    .filter((point) => Number.isFinite(point.time) && Number.isFinite(point.value) && point.time >= start && point.time <= end)
    .sort((a, b) => a.time - b.time);
  const segments: PlotPoint[][] = [];
  let previous = -Infinity;
  let previousPoint:typeof valid[number]|undefined;
  const position=(time:number,value:number):PlotPoint=>({
    x:((time-start)/(end-start))*WIDTH,
    y:HEIGHT-((Math.min(maxValue,Math.max(minValue,value))-minValue)/(maxValue-minValue))*HEIGHT,
    value,
  });
  for (const point of valid) {
    if (point.time - previous > gapMs) segments.push([]);
    else if(previousPoint&&splitAt.length){
      // Split before clamping so crossings at 70/95 remain at their real time,
      // even when the next reading is outside the fixed visible axis.
      const prior=previousPoint;
      const cuts=splitAt.filter(value=>value>Math.min(prior.value,point.value)&&value<Math.max(prior.value,point.value))
        .sort((a,b)=>point.value>prior.value?a-b:b-a);
      for(const value of cuts)segments[segments.length-1].push(position(prior.time+(point.time-prior.time)*(value-prior.value)/(point.value-prior.value),value));
    }
    segments[segments.length - 1].push(position(point.time,point.value));
    previous = point.time;
    previousPoint=point;
  }
  return segments;
}

function aboveRangeSegments(segments:PlotPoint[][]){
  return segments.flatMap(segment=>{
    const result:PlotPoint[][]=[];let active:PlotPoint[]=[];
    for(let index=0;index<segment.length;index++){
      const point=segment[index];
      if(point.value>TEMPERATURE_AXIS_MAX_C){
        if(!active.length&&index>0)active.push(segment[index-1]);
        active.push(point);
      }else if(active.length){active.push(point);result.push(active);active=[];}
    }
    if(active.length)result.push(active);
    return result;
  });
}

function line(points: { x: number; y: number }[]) {
  return points.map((point, index) => `${index === 0 ? "M" : "L"}${point.x.toFixed(1)},${point.y.toFixed(1)}`).join(" ");
}

export function HistoryChart({ title, headline, detail, detailTitle, series, maxValue, maxLabel, temperature, windowEnd, windowSeconds = 600, gapSeconds = DEFAULT_GAP_MS / 1_000, emptyLabel = "等待历史采样",rangeLabel,timeFormatter }: HistoryChartProps) {
  const hotClipId = useId().replace(/[^a-zA-Z0-9_-]/g, "");
  const end = Date.parse(windowEnd ?? "") || Date.now();
  const start = end - windowSeconds * 1_000;
  const scale = Number.isFinite(maxValue) && maxValue > 0 ? maxValue : 1;
  const rendered = series.map((item) => ({ ...item, segments: segmentPoints(item.points, start, end, scale, gapSeconds * 1_000) }));
  const thermalScale = temperature ? temperatureRange(temperature.points, start, end)??{min:TEMPERATURE_AXIS_MIN_C,max:TEMPERATURE_AXIS_MAX_C} : null;
  const thermalSegments = temperature && thermalScale ? segmentPoints(temperature.points.filter(point=>validTemperature(point.value)), start, end, thermalScale.max, temperature.gapSeconds * 1000, thermalScale.min,[thermalScale.min,HOT_TEMPERATURE_C,thermalScale.max]) : [];
  const aboveRange=aboveRangeSegments(thermalSegments);
  const hotThresholdY = thermalScale ? HEIGHT - ((HOT_TEMPERATURE_C - thermalScale.min) / (thermalScale.max - thermalScale.min)) * HEIGHT : null;
  const hasHotSegment = hotThresholdY !== null && thermalSegments.some(segment => segment.some(point => point.y < hotThresholdY));
  const thermalSampleCount = thermalSegments.reduce((count, segment) => count + segment.length, 0);
  const sampleCount = Math.max(0, ...rendered.map((item) => item.segments.reduce((total, segment) => total + segment.length, 0)));
  return <div className="history-chart">
    <div className="history-chart__heading">
      <div className="history-chart__heading-meta">
        <span title={detailTitle??detail}>{title}</span>
        <small title={detailTitle??detail}>{detail}</small>
      </div>
      <strong className="history-chart__headline">
        {headline}
        {temperature && <span className={`history-chart__temperature${isHotTemperature(temperature.value) ? " history-chart__temperature--hot" : ""}${validTemperature(temperature.value)&&temperature.value>TEMPERATURE_AXIS_MAX_C?" history-chart__temperature--critical":""}`} title={temperature.detail}>{formatTemperature(temperature.value)}</span>}
      </strong>
    </div>
    <div className="history-chart__plot">
      <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} preserveAspectRatio="none" role="img" aria-label={`${title}${rangeLabel??`近 ${Math.round(windowSeconds / 60)} 分钟`}真实采样曲线${temperature ? "，右轴温度固定 30–95°C，超过 70°C 的线段为红色，超过 95°C 的部分在顶边显示特粗红线" : ""}`}>
        <title>{`${title} · ${rangeLabel??`近 ${Math.round(windowSeconds / 60)} 分钟`}${temperature ? " · 温度轴 30–95°C；超过 70°C 变红，超过 95°C 贴顶加粗；读数为实际温度" : ""}`}</title>
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
        {aboveRange.map((segment,index)=><g key={`temperature-above-range-${index}`}>
          <path className="history-chart__temperature-above-range" d={line(segment)} fill="none" stroke="var(--chart-critical)" strokeWidth="12" vectorEffect="non-scaling-stroke" strokeLinejoin="round" strokeLinecap="butt"/>
          {segment.length===1&&<circle cx={Math.max(12,Math.min(WIDTH-12,segment[0].x))} cy="0" r="12" fill="var(--chart-critical)"/>}
        </g>)}
      </svg>
      <span className={`history-chart__max${temperature ? " history-chart__max--primary" : ""}`}>{maxLabel}</span>
      {temperature && <span className="history-chart__thermal-max">95°C</span>}
      <span className={`history-chart__zero${temperature ? " history-chart__zero--primary" : ""}`}>0</span>
      {temperature && <span className="history-chart__thermal-min">30°C</span>}
      {sampleCount === 0 && thermalSampleCount === 0 && <div className="history-chart__empty">{emptyLabel}</div>}
    </div>
    <div className="history-chart__footer">
      <time dateTime={new Date(start).toISOString()}>{timeFormatter?timeFormatter(start):clockFormatter.format(start)}</time>
      <div className="history-chart__legend">{rendered.map((item) => <span key={item.label}><i style={{ background: item.color }} />{item.label}</span>)}{temperature && <span title={temperature.detail}><i style={{ background: "var(--chart-blue)" }} />{title === "磁盘 IO" ? "SSD 温度" : "温度"}</span>}{aboveRange.length>0&&<span><i style={{background:"var(--chart-critical)"}}/>温度 &gt;95°C</span>}</div>
      <time dateTime={new Date(end).toISOString()}>{timeFormatter?timeFormatter(end):clockFormatter.format(end)}</time>
    </div>
  </div>;
}
