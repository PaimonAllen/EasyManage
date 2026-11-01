import {useEffect,useState} from "react";
import {fetchUsageHistory,type UsageHistory as History,type UsageStats,type UsageStat} from "../api/usageHistory";
import {HistoryChart} from "../components/HistoryChart";
import {formatBytes,formatRate} from "../lib/format";
import {usageOverheating,usagePoints,usageScale,usageTimeLabel,usageTimezone} from "../lib/usageHistory";
import "./UsageHistory.css";

const CYAN="var(--chart-cyan)",VIOLET="var(--chart-violet)",AMBER="var(--chart-amber)";
const percent=(value:number|null|undefined)=>value==null?"—":`${value.toFixed(1)}%`;
const bytes=(value:number|null|undefined)=>value==null?"—":formatBytes(value);
const rate=(value:number|null|undefined)=>value==null?"—":formatRate(value);

export default function UsageHistory(){
  const [node]=useState(()=>new URLSearchParams(window.location.search).get("node")??"");
  const [offset]=useState(()=>-new Date().getTimezoneOffset());
  const [data,setData]=useState<History|null>(null);const [error,setError]=useState("");const [busy,setBusy]=useState(false);const [revision,setRevision]=useState(0);
  const [period,setPeriod]=useState("all");const [gpu,setGpu]=useState("all");
  const [theme,setTheme]=useState(()=>{try{return localStorage.getItem("easymanage-theme")==="light"?"light":"dark";}catch{return "dark";}});
  useEffect(()=>{document.documentElement.dataset.theme=theme;},[theme]);
  useEffect(()=>{
    if(!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(node)){setError("机器 ID 无效，请从设备总览打开历史记录。");return;}
    const controller=new AbortController();
    // Defer the first load so StrictMode's discarded mount cannot start a
    // second expensive request. No interval or visibility-triggered polling.
    const timer=setTimeout(()=>{setBusy(true);setError("");
      void fetchUsageHistory(node,offset,controller.signal).then(value=>{if(!controller.signal.aborted){setData(value);setPeriod(current=>current==="all"||value.days.some(day=>day.date===current)?current:"all");setGpu(current=>current==="all"||value.gpus.some(item=>String(item.index)===current)?current:"all");document.title=`${value.hostname} · 历史使用记录`;}})
        .catch((e:unknown)=>{if(!controller.signal.aborted)setError(e instanceof Error?e.message:"历史读取失败");})
        .finally(()=>{if(!controller.signal.aborted)setBusy(false);});
    },0);
    return()=>{clearTimeout(timer);controller.abort();};
  },[node,offset,revision]);
  return <div className="dashboard usage-history">
    <header className="topbar"><a className="brand" href="/#/overview"><span>E</span><strong>EasyManage</strong></a><div className="topbar__right"><a className="button button--secondary button--small" href="/#/overview">设备总览</a><button type="button" className="theme-toggle" onClick={()=>setTheme(value=>value==="dark"?"light":"dark")}>{theme==="dark"?"浅色":"深色"}</button></div></header>
    <main className="usage-history__body">
      <header className="usage-history__heading"><div><h1>{data?.hostname??"机器"} · 历史使用记录</h1><p>近 7 个自然日（含今天） · {usageTimezone(offset)} · 读取已保存数据 · 按需查看</p></div><button type="button" className="button button--secondary" disabled={busy} onClick={()=>setRevision(value=>value+1)}>{busy?"读取中…":"重新读取"}</button></header>
      <p className="hint">每 10 分钟汇总均值与峰值，缺失记录留空。历史结果最多每 5 分钟更新一次，本页面不自动刷新。Overheating 仅表示硬件报告热降频；70°C 红色曲线只表示温度，不据此判定过热。旧历史和不支持的设备无法确认热降频。</p>
      {error&&<p className="form-error" role="alert">{error}</p>}{busy&&<p role="status">正在读取历史数据…</p>}
      {data&&<>
        <nav className="usage-history__periods" aria-label="历史日期"><button className="button button--secondary button--small" aria-pressed={period==="all"} onClick={()=>setPeriod("all")}>7 天总览</button>{data.days.map(day=><button key={day.date} className="button button--secondary button--small" aria-pressed={period===day.date} onClick={()=>setPeriod(day.date)}>{day.date.slice(5)}{day.date===data.days.at(-1)?.date?"（今天）":""}</button>)}</nav>
        <label className="usage-history__gpu-filter">GPU 视图<select value={gpu} onChange={e=>setGpu(e.target.value)}><option value="all">全部 GPU 汇总</option>{data.gpus.map(item=><option key={item.index} value={String(item.index)}>GPU {item.index} · {item.name}</option>)}</select></label>
        <UsageHistoryCharts data={data} period={period} gpu={gpu}/>
        <div className="data-panel usage-history__daily"><h2>每日使用汇总</h2><div className="table-scroll"><table><thead><tr><th>日期</th><th>CPU 均值 / 峰值</th><th>内存均值 / 峰值</th><th>GPU 核心均值 / 峰值</th><th>有效 GPU 数</th><th>CPU 有效样本</th><th>过热记录</th></tr></thead><tbody>{data.days.map(day=>{const g=gpu==="all"?day.summary.gpu:day.summary.gpus[gpu];return <tr key={day.date}><td><button type="button" className="usage-history__day" onClick={()=>setPeriod(day.date)}>{day.date}</button></td><td>{percent(day.summary.cpu.average)} / {percent(day.summary.cpu.maximum)}</td><td>{bytes(day.summary.memory.average)} / {bytes(day.summary.memory.maximum)}</td><td>{percent(g?.core.average)} / {percent(g?.core.maximum)}</td><td>{gpuCount(day.summary.gpu_count)}</td><td>{day.summary.cpu.samples.toLocaleString()}</td><td><UsageOverheatingNotice stats={day.summary} gpu={gpu} compact/></td></tr>;})}</tbody></table></div></div>
        <p className="hint">数据生成于 {usageTimeLabel(Date.parse(data.generated_at),offset,true)} · 本次读取 {data.source_rows.toLocaleString()} 条已保存上报。均值按有效采样计算，不把离线或缺失补成 0；不包含未写入历史的每秒实时通道。显存容量以当次采样为准，主机内存显示实际使用字节。GPU 编号来自历史记录，已不再上报的显卡仍可选择；有效 GPU 数不包含采集失败的空报。</p>
      </>}
    </main>
  </div>;
}
function gpuCount(stat:UsageStat){return stat.minimum===null||stat.maximum===null?"—":stat.minimum===stat.maximum?`${stat.maximum} 张`:`${stat.minimum}–${stat.maximum} 张`;}

export function UsageOverheatingNotice({stats,gpu="all",compact=false}:{stats:UsageStats;gpu?:string;compact?:boolean}){
  const readings=usageOverheating(stats,gpu);
  if(!readings.length){
    const known=(stats.cpu_thermal_throttling?.samples??0)+((gpu==="all"?stats.gpu:stats.gpus[gpu])?.thermal_throttling?.samples??0)>0;
    return compact?<span title="仅按已采集的硬件热降频证据判断；没有数据的部件无法确认">{known?"未记录热降频":"未采集降频状态"}</span>:null;
  }
  const detail=readings.map(reading=>`${reading.label} · ${reading.detected.toLocaleString()} 个热降频采样`).join("；");
  return compact?<span className="usage-history__overheating" title={`当日硬件曾报告热降频；${detail}`}><span aria-hidden="true">⚠ </span>Overheating · {detail}</span>
    :<div className="usage-history__overheating usage-history__overheating--notice" role="status"><strong><span aria-hidden="true">⚠ </span>Overheating</strong><span>本时段硬件曾报告热降频 · {detail}</span></div>;
}

export function UsageHistoryCharts({data,period="all",gpu="all"}:{data:History;period?:string;gpu?:string}){
  const day=data.days.find(item=>item.date===period);
  const start=day?.start??data.start,end=day?.end??data.end;
  const summary=day?.summary??data.summary;
  const points=data.points.filter(point=>Date.parse(point.at)>=Date.parse(start)&&Date.parse(point.at)<Date.parse(end));
  const pickGpu=(stats:UsageStats)=>gpu==="all"?stats.gpu:stats.gpus[gpu];
  const g=pickGpu(summary);
  const plot=(pick:(stats:UsageStats)=>UsageStat|undefined,field:"average"|"maximum"|"minimum"="average",scale=1)=>usagePoints(points,pick,field,scale);
  const common={windowEnd:end,windowSeconds:Math.max(1,(Date.parse(end)-Date.parse(start))/1000),gapSeconds:data.bucket_seconds*1.5,
    rangeLabel:day?day.date:"近 7 天",timeFormatter:(time:number)=>usageTimeLabel(time,data.utc_offset_minutes,!day),emptyLabel:"本时段没有有效采样"};
  const temperature=(pick:(stats:UsageStats)=>UsageStat|undefined,label:string)=>({points:plot(pick,"maximum"),value:pick(summary)?.maximum??null,detail:`${label} · 每个汇总区间的最高温度`,gapSeconds:common.gapSeconds});
  const memoryMax=usageScale(points.map(p=>p.stats.memory.maximum==null?null:p.stats.memory.maximum/1024**3));
  const diskMax=usageScale(points.flatMap(p=>[p.stats.disk_read.average,p.stats.disk_write.average]),1024);
  const networkMax=usageScale(points.flatMap(p=>[p.stats.network_rx.average,p.stats.network_tx.average]),1024);
  const countMax=Math.max(1,summary.gpu_count.maximum??1);
  return <>
    <UsageOverheatingNotice stats={summary} gpu={gpu}/>
    <div className="usage-history__summary">
      <div><span>CPU 均值 / 峰值</span><strong>{percent(summary.cpu.average)} / {percent(summary.cpu.maximum)}</strong></div>
      <div><span>内存均值 / 峰值</span><strong>{bytes(summary.memory.average)} / {bytes(summary.memory.maximum)}</strong></div>
      <div><span>{gpu==="all"?"GPU 核心均值 / 峰值":`GPU ${gpu} 核心均值 / 峰值`}</span><strong>{percent(g?.core.average)} / {percent(g?.core.maximum)}</strong></div>
      <div><span>显存均值 / 峰值</span><strong>{bytes(g?.memory.average)} / {bytes(g?.memory.maximum)}</strong></div>
      <div><span>有效 GPU 数</span><strong>{gpuCount(summary.gpu_count)}</strong></div>
    </div>
    <div className="usage-history__charts">
      <HistoryChart {...common} title="CPU" headline={`${percent(summary.cpu.average)} 均值`} detail="CPU 整机占比 · 均值 / 峰值" maxValue={100} maxLabel="100%" temperature={temperature(s=>s.cpu_temperature,"CPU 温度")}
        series={[{label:"均值",color:CYAN,points:plot(s=>s.cpu)},{label:"峰值",color:AMBER,points:plot(s=>s.cpu,"maximum")}]}/>
      <HistoryChart {...common} title="内存" headline={bytes(summary.memory.average)} detail="实际使用 · 均值 / 峰值" maxValue={memoryMax} maxLabel={`${memoryMax} GiB`}
        series={[{label:"均值",color:VIOLET,points:plot(s=>s.memory,"average",1024**3)},{label:"峰值",color:AMBER,points:plot(s=>s.memory,"maximum",1024**3)}]}/>
      <HistoryChart {...common} title={gpu==="all"?"GPU":`GPU ${gpu}`} headline={<span className="metric-dual-headline metric-dual-headline--gpu"><span>核心 {percent(g?.core.average)} · 显存 {percent(g?.memory_percent.average)}</span><span>{bytes(g?.memory.average)}</span></span>}
        detail={gpu==="all"?"每次采样核心均值 / 显存合计":data.gpus.find(item=>String(item.index)===gpu)?.name??"历史 GPU"} maxValue={100} maxLabel="100%" temperature={temperature(s=>pickGpu(s)?.temperature,"GPU 温度")}
        series={[{label:"核心均值",color:CYAN,points:plot(s=>pickGpu(s)?.core)},{label:"核心峰值",color:AMBER,points:plot(s=>pickGpu(s)?.core,"maximum")},{label:"显存",color:VIOLET,points:plot(s=>pickGpu(s)?.memory_percent)}]}/>
      <HistoryChart {...common} title="磁盘 IO" headline={<span className="metric-dual-headline"><span>读 {rate(summary.disk_read.average)}</span><span>写 {rate(summary.disk_write.average)}</span></span>} detail="已保存设备汇总 · 平均速率" maxValue={diskMax} maxLabel={formatRate(diskMax)} temperature={temperature(s=>s.ssd_temperature,"SSD 温度")}
        series={[{label:"读取",color:CYAN,points:plot(s=>s.disk_read)},{label:"写入",color:AMBER,points:plot(s=>s.disk_write)}]}/>
      <HistoryChart {...common} title="网络吞吐" headline={<span className="metric-dual-headline"><span>↓ {rate(summary.network_rx.average)}</span><span>↑ {rate(summary.network_tx.average)}</span></span>} detail="整机网络 · 平均速率" maxValue={networkMax} maxLabel={formatRate(networkMax)}
        series={[{label:"接收",color:CYAN,points:plot(s=>s.network_rx)},{label:"发送",color:AMBER,points:plot(s=>s.network_tx)}]}/>
      <HistoryChart {...common} title="有效 GPU 数" headline={gpuCount(summary.gpu_count)} detail="有效指标数量 · 缺失留空" maxValue={countMax} maxLabel={`${countMax} 张`}
        series={[{label:"最少",color:CYAN,points:plot(s=>s.gpu_count,"minimum")},{label:"最多",color:AMBER,points:plot(s=>s.gpu_count,"maximum")}]}/>
    </div>
  </>;
}
