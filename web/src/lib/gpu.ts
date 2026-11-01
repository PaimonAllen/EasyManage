import { percent } from "./format.ts";
import { validTemperature } from "./temperature.ts";
import type { GpuMetric, MetricHistoryPoint } from "../types/monitoring.ts";

type Reading = Pick<GpuMetric, "utilization_percent" | "memory_used_bytes" | "memory_total_bytes">;
type Sample = Pick<MetricHistoryPoint, "collected_at" | "gpu_collected_at" | "gpus">;
type Point = {at:string;value:number};

export function gpuDevices<T extends {index:number}>(gpus:T[]){
  return [...new Map(gpus.filter(gpu=>Number.isInteger(gpu.index)&&gpu.index>=0).map(gpu=>[gpu.index,gpu])).values()].sort((a,b)=>a.index-b.index);
}

export function gpuReading(gpu:Reading) {
  const core=Number.isFinite(gpu.utilization_percent)&&gpu.utilization_percent>=0&&gpu.utilization_percent<=100?gpu.utilization_percent:null;
  const memoryKnown=Number.isFinite(gpu.memory_total_bytes)&&gpu.memory_total_bytes>0&&Number.isFinite(gpu.memory_used_bytes)&&gpu.memory_used_bytes>=0&&gpu.memory_used_bytes<=gpu.memory_total_bytes;
  return {core,memory:memoryKnown?percent(gpu.memory_used_bytes,gpu.memory_total_bytes):null,
    memoryUsed:memoryKnown?gpu.memory_used_bytes:null,memoryTotal:memoryKnown?gpu.memory_total_bytes:null};
}

export function gpuHistoryPoints(samples:Sample[],index:number) {
  // GPU array order can change between samples. Match the reported device
  // index, and deduplicate cached readings by their actual GPU collection time.
  const core=new Map<string,Point>();const memory=new Map<string,Point>();const temperature=new Map<string,Point>();
  for(const sample of samples){
    const gpu=sample.gpus.find(item=>item.index===index);
    const at=sample.gpu_collected_at??sample.collected_at;
    if(!gpu||!Number.isFinite(Date.parse(at)))continue;
    const reading=gpuReading(gpu);
    if(reading.core!==null)core.set(at,{at,value:reading.core});
    if(reading.memory!==null)memory.set(at,{at,value:reading.memory});
    if(validTemperature(gpu.temperature_celsius))temperature.set(at,{at,value:gpu.temperature_celsius});
  }
  const sorted=(points:Map<string,Point>)=>[...points.values()].sort((a,b)=>Date.parse(a.at)-Date.parse(b.at));
  return {core:sorted(core),memory:sorted(memory),temperature:sorted(temperature)};
}
