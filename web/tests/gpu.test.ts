import assert from "node:assert/strict";
import {test} from "node:test";
import {gpuReading,gpuHistoryPoints} from "../src/lib/gpu.ts";
import type {GpuMetric,MetricHistoryPoint} from "../src/types/monitoring.ts";

const gpu=(index:number,core:number,memory:number,temperature:number|null):GpuMetric=>({index,name:`GPU ${index}`,utilization_percent:core,memory_used_bytes:memory,memory_total_bytes:100,temperature_celsius:temperature,power_watts:null});
const sample=(at:string,gpus:GpuMetric[],gpuAt=at)=>({collected_at:at,gpu_collected_at:gpuAt,gpus}) as MetricHistoryPoint;

test("per-GPU history follows device indices when reported array order changes",()=>{
  const a="2026-11-01T08:00:00Z",b="2026-11-01T08:00:10Z";
  const history=[sample(a,[gpu(0,10,20,45),gpu(1,80,90,75)]),sample(b,[gpu(1,95,70,81),gpu(0,30,40,48)])];
  const first=gpuHistoryPoints(history,0),second=gpuHistoryPoints(history,1);
  assert.deepEqual(first.core.map(p=>p.value),[10,30]);assert.deepEqual(first.memory.map(p=>p.value),[20,40]);
  assert.deepEqual(first.temperature.map(p=>p.value),[45,48]);
  assert.deepEqual(second.core.map(p=>p.value),[80,95]);assert.deepEqual(second.temperature.map(p=>p.value),[75,81]);
  assert.deepEqual(gpuHistoryPoints(history,2),{core:[],memory:[],temperature:[]});
});
test("cached GPU samples retain actual timestamps and legacy temperature fallback",()=>{
  const a="2026-11-01T08:00:00Z",b="2026-11-01T08:00:10Z";
  const history=[sample(b,[gpu(0,30,40,null)]),sample(a,[gpu(0,10,20,45)]),
    sample("2026-11-01T08:00:15Z",[gpu(0,30,40,48)],b),sample("2026-11-01T08:00:20Z",[gpu(0,30,40,null)],b)];
  const points=gpuHistoryPoints(history,0);
  assert.deepEqual(points.core,[{at:a,value:10},{at:b,value:30}]);
  assert.deepEqual(points.temperature,[{at:a,value:45},{at:b,value:48}]);
  const withoutTimestamp={collected_at:a,gpus:[gpu(0,0,0,0)]} as MetricHistoryPoint;
  assert.deepEqual(gpuHistoryPoints([withoutTimestamp],0).core,[{at:a,value:0}]);
  assert.equal(gpuHistoryPoints([sample('invalid',[gpu(0,10,10,45)])],0).core.length,0);
});
test("missing GPU values remain unknown independently of available counters",()=>{
  assert.deepEqual(gpuReading({...gpu(0,36,0,null),memory_total_bytes:0}),{core:36,memory:null,memoryUsed:null,memoryTotal:null});
  assert.equal(gpuReading(gpu(0,NaN,50,null)).memory,50);
  const missing=gpuHistoryPoints([sample('2026-11-01T08:00:00Z',[gpu(0,NaN,-1,null)])],0);
  assert.deepEqual(missing,{core:[],memory:[],temperature:[]});
  assert.equal(gpuReading(gpu(0,101,101,null)).memory,null);
});
