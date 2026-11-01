import assert from "node:assert/strict";
import {test} from "node:test";
import {usageOverheating,usagePoints,usageScale,usageTimeLabel,usageTimezone} from "../src/lib/usageHistory.ts";
import type {UsagePoint,UsageStats} from "../src/api/usageHistory.ts";
const stat=(value:number|null)=>({average:value,maximum:value,minimum:value,samples:value===null?0:1});
test("history plots preserve real idle samples and leave missing or disappeared GPUs empty",()=>{
  const points=[0,null,25].map((value,index)=>({at:`2026-11-01T08:${String(index*10).padStart(2,'0')}:00Z`,stats:{cpu:stat(value),gpus:index===0?{'7':{core:stat(80)}}:{}} as unknown as UsageStats}));
  assert.deepEqual(usagePoints(points,s=>s.cpu).map(p=>p.value),[0,25]);
  assert.deepEqual(usagePoints(points,s=>s.gpus['7']?.core).map(p=>p.value),[80]);
  const bytes=[{at:points[0].at,stats:{memory:stat(4*1024**3)} as UsageStats}] as UsagePoint[];
  assert.equal(usagePoints(bytes,s=>s.memory,'maximum',1024**3)[0].value,4);
});
test("history labels use the requested fixed offset across calendar days",()=>{
  assert.equal(usageTimezone(480),'UTC+08:00');assert.equal(usageTimezone(-330),'UTC-05:30');
  assert.match(usageTimeLabel(Date.parse('2026-10-31T16:00:00Z'),480,true),/11\/01.*00:00/);
  assert.equal(usageTimeLabel(Date.parse('2026-10-31T16:00:00Z'),480,false),'00:00');
  assert.equal(usageScale([null,NaN,0],1024),2000);assert.equal(usageScale([5,17]),20);
});
test("overheating requires hardware thermal-throttle evidence, irrespective of temperature, and follows GPU selection",()=>{
  const knownClear={samples:5,detected_samples:0};
  const detected={samples:5,detected_samples:1};
  const stats={cpu_temperature:stat(100),ssd_temperature:stat(99),cpu_thermal_throttling:knownClear,
    gpu:{temperature:stat(99),thermal_throttling:knownClear},
    gpus:{'0':{temperature:stat(100),thermal_throttling:knownClear},'7':{temperature:stat(60),thermal_throttling:detected}},
  } as unknown as UsageStats;
  assert.deepEqual(usageOverheating(stats),[]);
  assert.deepEqual(usageOverheating(stats,'0'),[]);
  assert.deepEqual(usageOverheating(stats,'7'),[{label:'GPU 7',detected:1}]);
  assert.deepEqual(usageOverheating({...stats,cpu_thermal_throttling:detected},'7'),[{label:'CPU',detected:1},{label:'GPU 7',detected:1}]);
  const old={...stats,cpu_thermal_throttling:undefined,gpu:{temperature:stat(120)}} as UsageStats;
  assert.deepEqual(usageOverheating(old),[]);
  assert.deepEqual(usageOverheating(old,'3'),[]);
});
