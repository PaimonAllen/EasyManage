import type {CSSProperties} from "react";
import type {SpaceScan} from "../api/diskSpace";

export function ScanProgress({scan}:{scan:SpaceScan}){
  const running=scan.status==="running";
  const complete=scan.status==="complete";
  const raw=running?(scan.progress.estimated_percent??scan.result?.estimated_percent):(scan.result?.estimated_percent??scan.progress.estimated_percent);
  const known=typeof raw==="number"&&Number.isFinite(raw);
  const percent=complete?100:known?Math.floor(Math.max(0,Math.min(99.9,raw))*10)/10:0;
  const label=complete?"100%":known?"预计 "+percent.toFixed(1)+"%":running?"正在估算…":"未提供进度估算";
  return <span className="disk-explorer__estimated-progress" title="按目录遍历位置估算；各子目录大小不同，百分比不代表精确剩余时间。">
    <progress value={percent} max={100} aria-label="扫描预计完成进度" aria-valuetext={scan.paused?"已暂停，"+label:label}/>
    <span className="disk-explorer__estimated-percent">{label}</span>
    {running&&<span className={"disk-explorer__loading-dots"+(scan.paused?" disk-explorer__loading-dots--paused":"")} aria-hidden="true">{[0,1,2,3,4].map(dot=><i key={dot} style={{"--dot":dot} as CSSProperties}/>)}</span>}
  </span>;
}
