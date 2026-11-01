import type {UsagePoint,UsageStat,UsageStats} from "../api/usageHistory.ts";

export function usageOverheating(stats:UsageStats,gpu="all"){
  const selectedGpu=gpu==="all"?stats.gpu:stats.gpus[gpu];
  return [
    {label:"CPU",detected:stats.cpu_thermal_throttling?.detected_samples??0},
    {label:gpu==="all"?"GPU":`GPU ${gpu}`,detected:selectedGpu?.thermal_throttling?.detected_samples??0},
  ].filter(reading=>reading.detected>0);
}

export function usagePoints(points:UsagePoint[],pick:(stats:UsageStats)=>UsageStat|undefined,field:"average"|"maximum"|"minimum"="average",scale=1){
  return points.flatMap(point=>{const value=pick(point.stats)?.[field];return typeof value==="number"&&Number.isFinite(value)?[{at:point.at,value:value/scale}]:[];});
}
export function usageScale(values:(number|null|undefined)[],minimum=1){
  const peak=Math.max(minimum,...values.filter((v):v is number=>typeof v==="number"&&Number.isFinite(v)&&v>=0));
  const power=10**Math.floor(Math.log10(peak));
  return ([1,2,5,10].find(factor=>factor*power>=peak)??10)*power;
}
export function usageTimeLabel(time:number,offset:number,date:boolean){
  return new Intl.DateTimeFormat("zh-CN",{timeZone:"UTC",...(date?{month:"2-digit",day:"2-digit"} as const:{}),hour:"2-digit",minute:"2-digit",hourCycle:"h23"}).format(time+offset*60_000);
}
export function usageTimezone(offset:number){return `UTC${offset<0?"-":"+"}${String(Math.floor(Math.abs(offset)/60)).padStart(2,"0")}:${String(Math.abs(offset)%60).padStart(2,"0")}`;}
