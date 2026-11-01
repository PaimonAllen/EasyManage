import type {CpuSocketMetric,MetricsSnapshot} from "../types/monitoring.ts";
import {validTemperature} from "./temperature.ts";

export function cpuSockets(sockets:CpuSocketMetric[]|undefined){
  return [...new Map((sockets??[]).filter(cpu=>Number.isInteger(cpu.id)&&cpu.id>=0&&cpu.logical_cpus>0).map(cpu=>[cpu.id,cpu])).values()].sort((a,b)=>a.id-b.id);
}
export function cpuSocketPoints(samples:Pick<MetricsSnapshot,"collected_at"|"fast_collected_at"|"temperatures"|"cpu_sockets">[],id:number){
  const usage=new Map<string,{at:string;value:number}>();const temperature=new Map<string,{at:string;value:number}>();
  for(const sample of samples){
    const cpu=sample.cpu_sockets?.find(cpu=>cpu.id===id);if(!cpu)continue;
    const at=sample.fast_collected_at??sample.collected_at;
    if(Number.isFinite(Date.parse(at))&&typeof cpu.usage_percent==="number"&&Number.isFinite(cpu.usage_percent)&&cpu.usage_percent>=0&&cpu.usage_percent<=100)usage.set(at,{at,value:cpu.usage_percent});
    const thermalAt=sample.temperatures?.collected_at;
    if(thermalAt&&Number.isFinite(Date.parse(thermalAt))&&validTemperature(cpu.temperature_celsius))temperature.set(thermalAt,{at:thermalAt,value:cpu.temperature_celsius});
  }
  const sorted=(points:Map<string,{at:string;value:number}>)=>[...points.values()].sort((a,b)=>Date.parse(a.at)-Date.parse(b.at));
  return {usage:sorted(usage),temperature:sorted(temperature)};
}
