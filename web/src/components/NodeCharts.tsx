import type { SensorSnapshot } from "../lib/temperature";
import { HistoryChart } from "./HistoryChart";
import { formatBytes, formatRate, formatUptime, percent } from "../lib/format";
import { hottest, ssdTemperature, temperaturePoints } from "../lib/temperature";
import { gpuReading, gpuHistoryPoints, gpuDevices } from "../lib/gpu";
import {cpuSockets,cpuSocketPoints} from "../lib/cpu";
import type { DiskMetric, GpuMetric, LiveNode, MetricHistoryPoint, NodeMetricHistory, NodeSummary } from "../types/monitoring";

const CYAN = "var(--chart-cyan)";
const VIOLET = "var(--chart-violet)";
const AMBER = "var(--chart-amber)";

type GpuReading = Pick<GpuMetric, "utilization_percent" | "memory_used_bytes" | "memory_total_bytes">;

function gpuUsage(gpus: GpuReading[]) {
  const valid = gpus.filter((gpu) => Number.isFinite(gpu.utilization_percent) && gpu.memory_total_bytes > 0);
  if (valid.length === 0) return null;
  const memoryUsed = valid.reduce((sum, gpu) => sum + gpu.memory_used_bytes, 0);
  const memoryTotal = valid.reduce((sum, gpu) => sum + gpu.memory_total_bytes, 0);
  return {
    core: valid.reduce((sum, gpu) => sum + gpu.utilization_percent, 0) / valid.length,
    memory: percent(memoryUsed, memoryTotal),
    memoryUsed,
    memoryTotal,
  };
}

function gpuMemoryCapacity(used: number, total: number) {
  const gib = 1024 ** 3;
  if (total >= gib) return `${(used / gib).toFixed(1)}/${(total / gib).toFixed(1)} GiB`;
  const mib = 1024 ** 2;
  return `${Math.round(used / mib)}/${Math.round(total / mib)} MiB`;
}

function rateScale(values: number[]) {
  const peak = Math.max(1024, ...values.filter((value) => Number.isFinite(value) && value >= 0));
  const power = 10 ** Math.floor(Math.log10(peak));
  return ([1, 2, 5, 10].find((factor) => factor * power >= peak) ?? 10) * power;
}

function diskRates(disks: DiskMetric[]) {
  const seen = new Set<string>();
  let read: number | null = null;
  let written: number | null = null;
  for (const disk of disks) {
    if (seen.has(disk.name)) continue;
    seen.add(disk.name);
    if (disk.read_bytes_per_second != null && Number.isFinite(disk.read_bytes_per_second) && disk.read_bytes_per_second >= 0)
      read = (read ?? 0) + disk.read_bytes_per_second;
    if (disk.written_bytes_per_second != null && Number.isFinite(disk.written_bytes_per_second) && disk.written_bytes_per_second >= 0)
      written = (written ?? 0) + disk.written_bytes_per_second;
  }
  return { read, written, devices: seen.size };
}

export function NodeCharts({ node, history, historyError, liveSamples, temperatureSnapshots = [], checkedAt, gpuExpanded=false, gpuDetailsId, cpuExpanded=false, cpuDetailsId }: { node: NodeSummary; history?: NodeMetricHistory; historyError?: string; liveSamples: LiveNode[]; temperatureSnapshots?: SensorSnapshot[]; checkedAt: number; gpuExpanded?:boolean; gpuDetailsId?:string;cpuExpanded?:boolean;cpuDetailsId?:string }) {
  const metrics = node.metrics;
  if (!metrics) return null;
  const cpus=cpuSockets(metrics.cpu_sockets);
  const gpus=gpuDevices(metrics.gpus);
  const latestLive = liveSamples.at(-1);
  const liveSeconds = metrics.collection_policy?.live_seconds ?? 1;
  const live = node.online && latestLive && checkedAt - Date.parse(latestLive.received_at) < (liveSeconds + 3) * 1000 ? latestLive.metrics : null;
  const livePoints: MetricHistoryPoint[] = liveSamples.map((sample) => ({
    collected_at: sample.metrics.collected_at,
    cpu_usage_percent: sample.metrics.cpu_usage_percent,
    memory_used_bytes: sample.metrics.memory_used_bytes,
    network_received_bytes_per_second: sample.metrics.network_received_bytes_per_second,
    network_transmitted_bytes_per_second: sample.metrics.network_transmitted_bytes_per_second,
    gpus: [],
  }));
  const points = [...(history?.points ?? []), ...livePoints];
  const fastPoints = Array.from(new Map(points.map((point) => {
    const at = point.fast_collected_at ?? point.collected_at;
    return [at, { ...point, collected_at: at }] as const;
  })).values());
  const common = { windowEnd: new Date(checkedAt).toISOString(), windowSeconds: 10 * 60 };
  const rx = live?.network_received_bytes_per_second ?? metrics.networks.reduce((sum, item) => sum + item.received_bytes_per_second, 0);
  const tx = live?.network_transmitted_bytes_per_second ?? metrics.networks.reduce((sum, item) => sum + item.transmitted_bytes_per_second, 0);
  const memoryUsed = live?.memory_used_bytes ?? metrics.memory_used_bytes;
  const memoryTotal = live?.memory_total_bytes ?? metrics.memory_total_bytes;
  const cpu = live?.cpu_usage_percent ?? metrics.cpu_usage_percent;
  const frequency=metrics.cpu_frequency;
  const frequencyAt=Date.parse(frequency?.collected_at??"");
  const frequencyFresh=node.online&&Number.isFinite(frequencyAt)&&frequencyAt<=checkedAt&&checkedAt-frequencyAt<=90_000;
  const hasFrequency=frequencyFresh&&frequency&&Number.isFinite(frequency.average_mhz)&&frequency.average_mhz>0;
  const frequencyLabel=hasFrequency?`均频 ${(frequency.average_mhz/1000).toFixed(2)} GHz`:"均频 —";
  const cpuDetail=`${cpus.length>1?`${cpus.length} 颗 CPU · `:""}${frequencyLabel} · ${node.cpu_logical_cores} 线程 · Load ${metrics.load_average_one.toFixed(2)}`;
  const cpuDetailTitle=hasFrequency?`${cpuDetail}；${frequency.logical_cpus} 个有效逻辑 CPU（含空闲），内核报告范围 ${(frequency.minimum_mhz/1000).toFixed(2)}–${(frequency.maximum_mhz/1000).toFixed(2)} GHz；采集于 ${new Date(frequency.collected_at).toLocaleString()}，30 秒缓存；频率变化不等同于热降频。`
    :`${cpuDetail}；${frequency?"频率数据已过期或节点离线":"暂无频率数据；旧 Agent 需更新，不支持的系统不会伪造频率"}。`;
  const networkMax = rateScale([rx, tx, ...points.flatMap((point) => [point.network_received_bytes_per_second, point.network_transmitted_bytes_per_second])]);
  const currentDisk = diskRates(metrics.disks);
  const diskSamples = Array.from(points.reduce((samples, point) => {
    if (point.disk_collected_at && (point.disk_read_bytes_per_second != null || point.disk_written_bytes_per_second != null))
      samples.set(point.disk_collected_at, {
        at: point.disk_collected_at,
        read: point.disk_read_bytes_per_second ?? null,
        written: point.disk_written_bytes_per_second ?? null,
      });
    return samples;
  }, new Map<string, { at: string; read: number | null; written: number | null }>()).values());
  const diskMax = rateScale([currentDisk.read ?? 0, currentDisk.written ?? 0, ...diskSamples.flatMap((sample) => [sample.read ?? 0, sample.written ?? 0])]);
  const currentGpu = gpuUsage(gpus);
  const gpuPoints = Array.from(new Map(points.flatMap((point) => {
    const usage = gpuUsage(gpuDevices(point.gpus));
    return usage ? [{ at: point.gpu_collected_at ?? point.collected_at, ...usage }] : [];
  }).map((point) => [point.at, point] as const)).values());
  const gpuDetail = currentGpu
    ? (gpus.length === 1 ? gpus[0].name : `${gpus.length} 块 GPU · 核心均值 / 显存合计`)
    : "当前未检测到 GPU 指标";
  const thermalSeconds = Math.max(30, metrics.collection_policy?.disk_seconds ?? 30);
  const thermalTime = metrics.temperatures?.collected_at;
  const thermalFresh = thermalTime && checkedAt - Date.parse(thermalTime) < (thermalSeconds * 2 + 30) * 1000;
  const gpuTime = metrics.gpu_collected_at ?? metrics.collected_at;
  const gpuSeconds = metrics.collection_policy?.gpu_seconds ?? 10;
  const gpuFresh = checkedAt - Date.parse(gpuTime) < (gpuSeconds * 2 + 30) * 1000;
  const temperature = (kind: "cpu" | "gpu" | "ssd") => ({
    points: temperaturePoints([...(history?.points ?? []), ...temperatureSnapshots], metrics, kind),
    value: kind === "gpu" ? (gpuFresh ? hottest(metrics.gpus.map(gpu => gpu.temperature_celsius)) : null)
      : !thermalFresh ? null : kind === "cpu" ? metrics.temperatures?.cpu_celsius ?? null : ssdTemperature(metrics.temperatures),
    gapSeconds: (kind === "gpu" ? gpuSeconds : thermalSeconds) * 2 + 15,
    detail: kind === "gpu" ? `GPU 最高温度 · ${gpuTime} · 温度轴固定 30–95°C，超出上限贴顶加粗，读数为实际温度`
      : `${kind === "cpu" ? "CPU 有效传感器最高温度" : "SSD 最高温度；" + (metrics.temperatures?.ssds.map(ssd => `${ssd.device}: ${ssd.temperature_celsius.toFixed(1)}°C`).join("、") || "暂无可读传感器")} · ${thermalTime ?? "等待 Agent 更新/采样"} · 温度轴固定 30–95°C，超出上限贴顶加粗，读数为实际温度`,
  });

  return <>
    <div className="overview-metrics">
      <HistoryChart {...common} temperature={temperature("cpu")} gapSeconds={Math.max(35, liveSeconds * 2 + 5)} title="CPU" headline={`${cpu.toFixed(1)}%`} detail={cpuDetail} detailTitle={cpuDetailTitle} maxValue={100} maxLabel="100%" series={[{ label: "使用率", color: CYAN, points: fastPoints.map((point) => ({ at: point.collected_at, value: point.cpu_usage_percent })) }]} />
      <HistoryChart {...common} gapSeconds={Math.max(35, liveSeconds * 2 + 5)} title="内存" headline={`${percent(memoryUsed, memoryTotal).toFixed(1)}%`} detail={`${formatBytes(memoryUsed)} / ${formatBytes(memoryTotal)}`} maxValue={100} maxLabel="100%" series={[{ label: "已用", color: VIOLET, points: fastPoints.map((point) => ({ at: point.collected_at, value: percent(point.memory_used_bytes, memoryTotal) })) }]} />
      <HistoryChart {...common} temperature={temperature("gpu")} gapSeconds={Math.max(35, (metrics.collection_policy?.gpu_seconds ?? 10) * 2 + 5)} title="GPU" headline={currentGpu ? <span className="metric-dual-headline metric-dual-headline--gpu"><span>核心 {currentGpu.core.toFixed(1)}% · 显存 {currentGpu.memory.toFixed(1)}%</span><span>{gpuMemoryCapacity(currentGpu.memoryUsed, currentGpu.memoryTotal)}</span></span> : "—"} detail={gpuDetail} maxValue={100} maxLabel="100%" emptyLabel={currentGpu ? "等待 GPU 历史采样" : "未检测到 GPU 历史指标"} series={[
        { label: "核心", color: CYAN, points: gpuPoints.map((point) => ({ at: point.at, value: point.core })) },
        { label: "显存", color: VIOLET, points: gpuPoints.map((point) => ({ at: point.at, value: point.memory })) },
      ]} />
      <HistoryChart {...common} temperature={temperature("ssd")} title="磁盘 IO" headline={<span className="metric-dual-headline"><span>读 {currentDisk.read == null ? "—" : formatRate(currentDisk.read)}</span><span>写 {currentDisk.written == null ? "—" : formatRate(currentDisk.written)}</span></span>} detail={metrics.disk_collected_at ? `${currentDisk.devices} 个设备` : "等待采样"} maxValue={diskMax} maxLabel={formatRate(diskMax)} gapSeconds={Math.max(75, (metrics.collection_policy?.disk_seconds ?? 30) * 2 + 15)} emptyLabel="等待两次磁盘采样" series={[
        { label: "读取", color: CYAN, points: diskSamples.flatMap((sample) => sample.read == null || !Number.isFinite(sample.read) ? [] : [{ at: sample.at, value: sample.read }]) },
        { label: "写入", color: AMBER, points: diskSamples.flatMap((sample) => sample.written == null || !Number.isFinite(sample.written) ? [] : [{ at: sample.at, value: sample.written }]) },
      ]} />
      <HistoryChart {...common} gapSeconds={Math.max(35, liveSeconds * 2 + 5)} title="网络吞吐" headline={<span className="metric-dual-headline"><span>↓ {formatRate(rx)}</span><span>↑ {formatRate(tx)}</span></span>} detail={`运行 ${formatUptime(metrics.uptime_seconds)}`} maxValue={networkMax} maxLabel={formatRate(networkMax)} series={[
        { label: "接收", color: CYAN, points: fastPoints.map((point) => ({ at: point.collected_at, value: point.network_received_bytes_per_second })) },
        { label: "发送", color: AMBER, points: fastPoints.map((point) => ({ at: point.collected_at, value: point.network_transmitted_bytes_per_second })) },
      ]} />
    </div>
    {cpus.length>1&&<section id={cpuDetailsId} className="node-cpu-details" hidden={!cpuExpanded} aria-label={`${node.hostname} CPU 详情`}>
      {cpuExpanded&&<>
        <div className="node-gpu-details__heading"><h3>CPU 详情</h3><span>{cpus.length} 颗物理 CPU · 近 10 分钟内本页面收到的真实采样 · 不增加请求</span></div>
        <div className="node-gpu-details__grid">{cpus.map(cpu=>{
          const samples=cpuSocketPoints([...temperatureSnapshots,metrics],cpu.id);
          const at=cpu.frequency?Date.parse(cpu.frequency.collected_at):NaN;
          const frequencyKnown=node.online&&Number.isFinite(at)&&at<=checkedAt&&checkedAt-at<=90_000&&cpu.frequency&&cpu.frequency.average_mhz>0;
          const frequencyLabel=frequencyKnown?`均频 ${(cpu.frequency!.average_mhz/1000).toFixed(2)} GHz`:"均频 —";
          return <HistoryChart key={cpu.id} {...common} title={`CPU ${cpu.id}`} detail={`${frequencyLabel} · ${cpu.logical_cpus} 线程${cpu.physical_cores?` / ${cpu.physical_cores} 核`:""}`}
            detailTitle={`${cpu.model||node.cpu_model} · 物理插槽 ${cpu.id} · ${cpu.frequency?`频率采集于 ${cpu.frequency.collected_at}，30 秒缓存`:"频率未采集"}`}
            headline={cpu.usage_percent==null?"—":`${cpu.usage_percent.toFixed(1)}%`} maxValue={100} maxLabel="100%" gapSeconds={Math.max(35,(metrics.collection_policy?.report_seconds??10)*2+5)} emptyLabel="等待此 CPU 的采样"
            temperature={{points:samples.temperature,value:thermalFresh?cpu.temperature_celsius:null,gapSeconds:thermalSeconds*2+15,detail:`仅显示可对应物理插槽 ${cpu.id} 的温度；不复用整机最高温度`}}
            series={[{label:"使用率",color:CYAN,points:samples.usage}]}/>;
        })}</div>
      </>}
    </section>}
    {gpus.length>1&&<section id={gpuDetailsId} className="node-gpu-details" hidden={!gpuExpanded} aria-label={`${node.hostname} GPU 详情`}>
      {gpuExpanded&&<>
        <div className="node-gpu-details__heading"><h3>GPU 详情</h3><span>{gpus.length} 块 GPU · 近 10 分钟 · 每张卡独立显示</span></div>
        <div className="node-gpu-details__grid">{gpus.map(gpu=>{
          const usage=gpuReading(gpu);
          const samples=gpuHistoryPoints([...(history?.points??[]),...temperatureSnapshots,metrics],gpu.index);
          return <HistoryChart key={gpu.index} {...common} title={`GPU ${gpu.index}`} detail={gpu.name}
            headline={<span className="metric-dual-headline metric-dual-headline--gpu"><span>核心 {usage.core===null?"—":`${usage.core.toFixed(1)}%`} · 显存 {usage.memory===null?"—":`${usage.memory.toFixed(1)}%`}</span><span>{usage.memoryUsed===null||usage.memoryTotal===null?"显存容量 —":gpuMemoryCapacity(usage.memoryUsed,usage.memoryTotal)}</span></span>}
            maxValue={100} maxLabel="100%" gapSeconds={Math.max(35,gpuSeconds*2+5)} emptyLabel="等待此 GPU 的历史采样"
            temperature={{points:samples.temperature,value:gpuFresh?hottest([gpu.temperature_celsius]):null,gapSeconds:gpuSeconds*2+15,
              detail:`GPU ${gpu.index} · ${gpu.name} · ${gpuTime} · 温度轴固定 30–95°C，超出上限贴顶加粗，读数为实际温度`}}
            series={[{label:"核心",color:CYAN,points:samples.core},{label:"显存",color:VIOLET,points:samples.memory}]}/>;
        })}</div>
      </>}
    </section>}
    {historyError && <p className="history-warning">{historyError}</p>}
  </>;
}
