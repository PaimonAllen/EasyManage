import assert from "node:assert/strict";
import {test} from "node:test";
import {cpuSockets,cpuSocketPoints} from "../src/lib/cpu.ts";
import {recordTemperatureSnapshots} from "../src/lib/temperature.ts";
import type {CpuSocketMetric,MetricsSnapshot,NodeSummary} from "../src/types/monitoring.ts";

const socket=(id:number,usage:number|null,temp:number|null=null):CpuSocketMetric=>({id,model:`Package ${id}`,logical_cpus:96,physical_cores:48,usage_percent:usage,frequency:null,temperature_celsius:temp});
test("multi-CPU means distinct physical packages, not 96 logical threads",()=>{
  assert.equal(cpuSockets([socket(0,10)]).length,1);
  assert.deepEqual(cpuSockets([socket(7,70),socket(2,20),socket(7,75)]).map(cpu=>cpu.id),[2,7]);
  assert.equal(cpuSockets(undefined).length,0);
});
test("CPU curves follow package IDs, deduplicate cached source times and leave missing sensors/devices empty",()=>{
  const at='2026-11-01T08:00:00Z',next='2026-11-01T08:00:20Z';
  const a={collected_at:at,fast_collected_at:at,temperatures:{collected_at:at,cpu_celsius:80,ssds:[]},cpu_sockets:[socket(7,70,null),socket(2,20,50)]};
  const b={...a,collected_at:'2026-11-01T08:00:10Z',cpu_sockets:[socket(2,20,50),socket(7,70,null)]};
  const c={...a,collected_at:next,fast_collected_at:next,cpu_sockets:[socket(2,25,50)]};
  assert.deepEqual(cpuSocketPoints([a,b,c],7).usage,[{at,value:70}]);
  assert.deepEqual(cpuSocketPoints([a,b,c],7).temperature,[],'whole-machine 80°C is never assigned to package 7');
  assert.deepEqual(cpuSocketPoints([a,b,c],2).usage,[{at,value:20},{at:next,value:25}]);
  assert.deepEqual(cpuSocketPoints([a,b,c],2).temperature,[{at,value:50}]);
});
test("existing bounded snapshot memory also retains CPU-only machines with no GPU or temperature sensors",()=>{
  const now=Date.parse('2026-11-01T08:00:30Z');
  const metric={collected_at:'2026-11-01T08:00:00Z',gpus:[],cpu_sockets:[socket(0,10),socket(1,20)]} as unknown as MetricsSnapshot;
  const node={id:'cpu-only',online:true,metrics:metric} as NodeSummary;
  let saved=recordTemperatureSnapshots({},[node],now);
  assert.equal(saved[node.id].length,1);
  saved=recordTemperatureSnapshots(saved,[node],now);
  assert.equal(saved[node.id].length,1,'same sampling time must not become a new point');
  saved=recordTemperatureSnapshots(saved,[{...node,metrics:{...metric,collected_at:'2026-11-01T08:00:10Z'}}],now);
  assert.equal(saved[node.id].length,2);
  assert.deepEqual(recordTemperatureSnapshots(saved,[{...node,online:false}],now+601_000),{});
});
