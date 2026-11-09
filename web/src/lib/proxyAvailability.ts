import type {ProxyAvailabilitySeries,ProxyLabNode,ReputationScore} from "../api/proxyLab";
// Existing round status identifies work awaiting a saved result, without new probes or polling.
export function pendingAvailabilitySlot(round:{started_at:string|null;finished_at:string|null}|undefined,lastCheck:string|null,enabled:boolean,now=Date.now()):string|null{
  if(!enabled||!round?.started_at||round.finished_at)return null;
  const started=Date.parse(round.started_at);
  if(!Number.isFinite(started)||started>now+5_000||now-started>=180_000)return null;
  const slot=Math.floor(started/180_000)*180_000;
  if(lastCheck&&Date.parse(lastCheck)>=slot)return null;
  return new Date(slot).toISOString();
}
function availabilitySample(code:string){
  switch(code){
    case "u":return {code,state:"up",label:"正常"};
    case "s":return {code,state:"slow",label:"缓慢"};
    case "d":return {code,state:"down",label:"故障"};
    case "x":return {code,state:"unknown",label:"暂不支持"};
    case "?":return {code,state:"unknown",label:"检测未确认"};
    default:return {code,state:"unknown",label:"未采集"};
  }
}
export function availabilityBuckets(history:string){
  const normalized=history.padStart(480,"-").slice(-480);
  return Array.from({length:96},(_,index)=>{
    const codes=normalized.slice(index*5,index*5+5);
    const up=[...codes].filter(c=>c==="u"||c==="s").length;
    const down=[...codes].filter(c=>c==="d").length;
    const slow=codes.includes("s");const known=up+down;
    return {index,up,down,known,unknown:5-known,samples:[...codes].map(availabilitySample),state:known===0?"unknown":down===0?(slow?"slow":"up"):up===0?"down":"mixed"};
  });
}
export function nodeEgress(node:ProxyLabNode):string|null{
  const scores=node.scores.filter(s=>s.revision===node.revision&&s.ip);
  const detected=scores.filter(s=>s.method==="api");
  return [...(detected.length?detected:scores)].sort((a,b)=>Date.parse(b.checked_at)-Date.parse(a.checked_at))[0]?.ip??null;
}
export function currentScore(node:ProxyLabNode,score:ReputationScore,now=Date.now()){
  const at=Date.parse(score.observed_at??score.checked_at);
  return score.revision===node.revision&&score.ip===nodeEgress(node)&&["ok","manual"].includes(score.state)
    &&score.purity!==null&&Number.isFinite(score.purity)&&Number.isFinite(at)&&now-at<=24*60*60_000&&at<=now+5*60_000;
}
export function reputationMean(node:ProxyLabNode,now=Date.now()){
  const scores=node.scores.filter(s=>currentScore(node,s,now));
  return scores.length?{value:scores.reduce((total,s)=>total+(s.purity??0),0)/scores.length,count:scores.length}:null;
}
export function nodeRegion(node:ProxyLabNode){
  const verified=node.scores.find(s=>s.method==="api"&&currentScore(node,s)&&s.region);
  return {region:verified?.region??node.region,verified:Boolean(verified)};
}

const EMPTY_SERIES:ProxyAvailabilitySeries={history:"-".repeat(480),latest:null,last_check:null,available_samples:0,observed_samples:0,expected_samples:480,availability_percent:null,coverage_percent:0};
export function availabilityView(node:ProxyLabNode,mode:"https"|"node_link"):ProxyAvailabilitySeries{
  return mode==="node_link"?(node.node_link??EMPTY_SERIES):node;
}
