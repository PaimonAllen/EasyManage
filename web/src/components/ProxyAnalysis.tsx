import {lazy,Suspense,memo,useCallback,useEffect,useMemo,useRef,useState} from "react";
import {fetchProxyLab,fetchProxyDetails,fetchProxySources,importProxyProfile,renameProxyProfile,deleteProxyProfile,scoreProxyNodes,fetchReputationJob,saveManualReputation,
  LOCAL_NETWORK_PROFILE,type ProxyProbeSite,type ProxyLabStatus,type ProxyLabDetail,type ProxyLabNode,type ProxyProfile,type ReputationProvider,type ReputationJob,type LocalProxySource} from "../api/proxyLab";
import {availabilityBuckets,availabilityView,pendingAvailabilitySlot,nodeEgress,nodeRegion,reputationMean,currentScore} from "../lib/proxyAvailability";
import {formatDate} from "../lib/format";
import "./proxyAnalysis.css";
import ProxyMonitoring,{type MonitoringChange} from "./ProxyMonitoring";
import {sameNameProfiles} from "../lib/proxyImport";

const FOCUS=[{id:"US",name:"美国",flag:"🇺🇸"},{id:"JP",name:"日本",flag:"🇯🇵"},{id:"SG",name:"新加坡",flag:"🇸🇬"},{id:"TW",name:"台湾",flag:"🇹🇼"}];
const ALL_PROFILES="all-profiles";
const LocalNetworkPanel=lazy(()=>import("./LocalNetworkPanel"));
export function ProxyScopeSelect({value,profiles,onChange,paused=false}:{value:string;profiles:ProxyProfile[];onChange:(value:string)=>void;paused?:boolean}){
  return <label>检测范围<select value={value}onChange={e=>onChange(e.target.value)}><option value="">请选择配置</option><option value={LOCAL_NETWORK_PROFILE}>本机网络检测 · 20 个网站</option><option value={ALL_PROFILES}>所有配置 · 所有节点</option>{profiles.map(p=><option key={p.id}value={p.id}>{p.name} · {p.node_count} 节点{p.enabled?(paused?" · 已启用 · 总暂停":" · 已启用检测"):" · 已暂停"}{(p.duplicate_count??1)>1?" · 内容重复":""}</option>)}</select></label>;
}
type ViewedNode=ProxyLabNode&{profileId:string;profileName:string;profileEnabled:boolean;windowStart:string};
const err=(e:unknown)=>e instanceof Error?e.message:"平台代理分析暂不可用";
const availabilityTime=new Intl.DateTimeFormat("zh-CN",{year:"numeric",month:"numeric",day:"numeric",hour:"numeric",minute:"numeric",second:"numeric",hour12:false});
const timeLabels=new Map<string,{from:string;points:string[]}[]>();
function availabilityTimes(start:string){
  const cached=timeLabels.get(start);if(cached)return cached;
  const epoch=Date.parse(start);
  const groups=Array.from({length:96},(_,index)=>{
    const from=epoch+index*15*60_000;
    return {from:availabilityTime.format(from),points:Array.from({length:5},(_,sample)=>availabilityTime.format(from+sample*3*60_000))};
  });
  if(timeLabels.size>=4)timeLabels.delete(timeLabels.keys().next().value!);
  timeLabels.set(start,groups);return groups;
}
export const ProxyAvailabilityBar=memo(function ProxyAvailabilityBar({history,start,label="过去24小时可用性",checkingAt=null}:{history:string;start:string;label?:string;checkingAt?:string|null}){
  const times=availabilityTimes(start);
  const checkingIndex=checkingAt?Math.floor((Date.parse(checkingAt)-Date.parse(start))/180_000):-1;
  const checking=checkingIndex>=0&&checkingIndex<480;
  return <div className="proxy-availability" role="img" aria-label={`${label}：竖向大格背景汇总15分钟，内部从上到下堆叠5条横向小格，各代表3分钟；绿色正常，黄色缓慢或混合，红色故障，灰色无有效观测${checking?"；闪烁星号表示本轮检测中，等待结果":""}`}>
    {availabilityBuckets(history).map(bucket=>{const time=times[bucket.index];
      const samples=bucket.samples.map((sample,index)=>`${time.points[index]}：${sample.label}`).join("\n");
      const colors=bucket.samples.map((sample,index)=>`var(--proxy-sample-${sample.state}) ${index*20}% ${(index+1)*20}%`).join(",");
      return <span key={bucket.index} className={`proxy-availability__slot proxy-availability__slot--${bucket.state}${bucket.known>0&&bucket.known<5?" proxy-availability__slot--partial":""}`}
        title={`${time.from} 起 15 分钟：可用 ${bucket.up} 次，故障 ${bucket.down} 次，未知 ${bucket.unknown} 次\n从上到下，每小格 3 分钟：\n${samples}${checking&&Math.floor(checkingIndex/5)===bucket.index?"\n✱ 本轮检测中，等待结果":""}`} aria-hidden="true">
        <span className="proxy-availability__samples" style={{backgroundImage:`linear-gradient(to bottom,${colors})`}}/>
        {checking&&Math.floor(checkingIndex/5)===bucket.index&&<span className="proxy-availability__checking" style={{top:`${(checkingIndex%5)*20+10}%`}}>✱</span>}
      </span>;})}
  </div>;
});
export function ProxyProbeResults({node,sites=[],fallbacks=[],defaultOpen=false}:{node:Pick<ProxyLabNode,"latest">;sites?:ProxyProbeSite[];fallbacks?:ProxyProbeSite[];defaultOpen?:boolean}){
  const observation=node.latest;
  if(!observation)return null;
  if((observation.policy??1)<2)return <p className="hint">最近一次为旧版单站检测；后续检测使用多站规则。</p>;
  const results=observation.sites??[];
  if(!results.length)return <p className="hint">多站检测未取得逐站结果；请查看节点连接或内核状态。</p>;
  const success=results.filter(ms=>ms>=0).length;const unknown=results.filter(ms=>ms===-2).length;
  return <details className="proxy-probe-details" open={defaultOpen}><summary>站点通过 {success}/{results.length} · {(success/results.length*100).toFixed(1)}%{unknown?` · ${unknown} 项未确认`:""}<small> · 检测明细</small></summary>
    <div className="proxy-probe-sites">{results.map((ms,index)=>{
      const site=sites[index];const feedback=observation.site_feedback?.find(value=>value.site===index);const backup=fallbacks[index];
      const feedbackLabel=feedback?.outcome==="fallback_ok"?"回退成功":feedback?.outcome==="primary_ok"?"补充核验通过":feedback?.outcome==="rate_limited"?"限流 / 冷却中":feedback?.outcome==="transport_failed"?"独立 HTTPS 复核及重试均未连通":feedback?.outcome==="tls_error"?"目标 HTTPS 证书校验失败":feedback?.outcome==="local_error"?"平台本地资源异常，未确认":feedback?.outcome==="http_restricted"?"网站访问受限 / 响应异常，未确认":feedback?.outcome==="fallback_failed"?"备用地址未通过":feedback?.outcome==="service_unavailable"?"网站服务暂不可用":"HTTP 响应未通过";
      return <div key={index} className={`proxy-probe-site proxy-probe-site--${ms>=0?"up":ms===-1?"down":feedback?.rate_limited?"limited":"unknown"}`}>
        {site?<a href={site.url} target="_blank" rel="noopener noreferrer" title={`HTTPS HEAD · 期望 HTTP ${site.expected}`}>{site.name} ↗</a>:<span>站点 {index+1}</span>}
        <strong>{ms>=0?`通过 · ${ms} ms`:ms===-1?"失败 / 超时":feedback?.rate_limited?"网站限流 · 未确认":feedback?.outcome==="http_restricted"?"网站受限 · 未确认":"证据未确认"}</strong>
        {feedback&&<><small className={feedback.rate_limited?"proxy-probe-site__warning":""}>{feedbackLabel}{feedback.primary_status?` · 原地址 HTTP ${feedback.primary_status}`:""}{feedback.primary_status===403&&!feedback.rate_limited?" · 访问受限，未认定限流":""}</small>
          {feedback.fallback_status!=null&&backup&&<small><a href={backup.url} target="_blank" rel="noopener noreferrer" title={backup.url}>{backup.name} ↗</a> · HTTP {feedback.fallback_status}</small>}
          {feedback.retry_at&&Number.isFinite(Date.parse(feedback.retry_at))&&Date.parse(feedback.retry_at)>Date.now()&&<small>原地址冷却至 {formatDate(feedback.retry_at)}</small>}
          {feedback.fallback_retry_at&&Number.isFinite(Date.parse(feedback.fallback_retry_at))&&Date.parse(feedback.fallback_retry_at)>Date.now()&&<small>备用地址冷却至 {formatDate(feedback.fallback_retry_at)}</small>}
        </>}
      </div>;
    })}</div><p className="hint">{(observation.policy??1)>=5?"达到 80% 即通过":"严格超过 80% 才通过"}：{results.length} 站至少 {(observation.policy??1)>=5?Math.ceil(results.length*0.8):Math.floor(results.length*0.8)+1} 站成功。{(observation.policy??1)>=4?"每站超时 8 秒；失败项经原节点独立复核，传输错误最多重试一次。":"每站超时 5 秒；"}仅正确 HTTPS 状态计成功，错误页和重定向不计。延迟取成功站点的中位数。限流或访问受限时最多回退一个同服务地址；原地址与备用地址合计只算一票。冷却期间不重复请求该地址；403、限流、网站服务异常与平台资源异常保留未确认，不伪装成访问成功或直接判为节点故障。</p>
  </details>;
}
export default function ProxyAnalysis(){
  const [status,setStatus]=useState<ProxyLabStatus>();const [selected,setSelected]=useState("");const [details,setDetails]=useState<ProxyLabDetail[]>([]);
  const [detailsLoaded,setDetailsLoaded]=useState(false);const [failedProfiles,setFailedProfiles]=useState<string[]>([]);
  const lastProfile=useRef("");const showAll=selected===ALL_PROFILES;
  const profileIds=status?.profiles.map(p=>p.id).join(",")??"";
  const [error,setError]=useState("");const [message,setMessage]=useState("");const [importing,setImporting]=useState(false);const [manual,setManual]=useState<ProxyLabNode>();
  const [jobId,setJobId]=useState("");const [job,setJob]=useState<ReputationJob>();const [busy,setBusy]=useState(false);const [revision,setRevision]=useState(0);
  const [search,setSearch]=useState("");const [order,setOrder]=useState("name");const [chosen,setChosen]=useState<string[]>([]);
  const [groupByRegion,setGroupByRegion]=useState(true);
  const [chartMode,setChartMode]=useState<"https"|"node_link">("https");
  const [showOther,setShowOther]=useState(false);
  const [renaming,setRenaming]=useState<ProxyProfile>();
  const [deleting,setDeleting]=useState<ProxyProfile>();
  const refresh=useCallback(()=>setRevision(r=>r+1),[]);
  useEffect(()=>{
    const controller=new AbortController();let pending=false;
    const load=async()=>{if(pending||document.hidden)return;pending=true;try{const value=await fetchProxyLab(AbortSignal.any([controller.signal,AbortSignal.timeout(12_000)]));if(!controller.signal.aborted)setStatus(value);}catch(e){if(!controller.signal.aborted)setError(err(e));}finally{pending=false;}};
    void load();const timer=window.setInterval(()=>void load(),30_000);document.addEventListener("visibilitychange",load);
    return()=>{controller.abort();window.clearInterval(timer);document.removeEventListener("visibilitychange",load);};
  },[revision]);
  useEffect(()=>{if(status&&selected!==LOCAL_NETWORK_PROFILE&&selected!==ALL_PROFILES&&!status.profiles.some(p=>p.id===selected))setSelected(status.profiles.find(p=>p.enabled)?.id??status.profiles[0]?.id??"");},[status,selected]);
  useEffect(()=>{setChosen([]);setDetails([]);setDetailsLoaded(false);setFailedProfiles([]);},[selected]);
  useEffect(()=>{
    if(!selected||selected===LOCAL_NETWORK_PROFILE)return;const controller=new AbortController();let pending=false;
    const ids=selected===ALL_PROFILES?profileIds.split(",").filter(Boolean):[selected];
    const load=async()=>{if(pending||document.hidden)return;pending=true;try{
      const result=await fetchProxyDetails(ids,controller.signal);if(controller.signal.aborted)return;
      setDetails(previous=>ids.flatMap(id=>{const value=result.details.find(d=>d.profile_id===id)??previous.find(d=>d.profile_id===id);return value?[value]:[];}));
      setFailedProfiles(result.failed);setDetailsLoaded(true);
    }catch(e){if(!controller.signal.aborted){setFailedProfiles(ids);setDetailsLoaded(true);setError(err(e));}}finally{pending=false;}};
    void load();const timer=window.setInterval(()=>void load(),30_000);document.addEventListener("visibilitychange",load);
    return()=>{controller.abort();window.clearInterval(timer);document.removeEventListener("visibilitychange",load);};
  },[selected,revision,profileIds]);
  useEffect(()=>{
    if(!jobId)return;const controller=new AbortController();let timer:number|undefined;
    const load=async()=>{try{const value=await fetchReputationJob(jobId,AbortSignal.any([controller.signal,AbortSignal.timeout(10_000)]));if(controller.signal.aborted)return;setJob(value);if(value.status==="running")timer=window.setTimeout(()=>void load(),2000);else{setJobId("");refresh();}}catch(e){if(!controller.signal.aborted){setError(err(e));setJobId("");refresh();}}};void load();
    return()=>{controller.abort();window.clearTimeout(timer);};
  },[jobId,refresh]);
  const profile=status?.profiles.find(p=>p.id===selected);
  const viewedNodes=useMemo<ViewedNode[]>(()=>details.flatMap(detail=>{
    const owner=status?.profiles.find(p=>p.id===detail.profile_id);
    if(!owner||(!showAll&&owner.id!==selected))return [];
    return detail.nodes.map(node=>({...node,profileId:owner.id,profileName:owner.name,profileEnabled:owner.enabled,windowStart:detail.window_start}));
  }),[details,status?.profiles,selected,showAll]);
  function selectScope(value:string){if(value!==ALL_PROFILES)lastProfile.current=value;setSelected(value);setShowOther(false);if(value===ALL_PROFILES)setGroupByRegion(false);}
  function toggleAll(){if(showAll)selectScope(status?.profiles.find(p=>p.id===lastProfile.current)?.id??status?.profiles[0]?.id??"");else{lastProfile.current=selected;selectScope(ALL_PROFILES);}}
  function removed(id:string,message:string){
    setStatus(previous=>previous?{...previous,profiles:previous.profiles.filter(p=>p.id!==id)}:previous);
    setDetails(previous=>previous.filter(d=>d.profile_id!==id));setFailedProfiles(previous=>previous.filter(value=>value!==id));
    setChosen(previous=>previous.filter(node=>!viewedNodes.some(n=>n.id===node&&n.profileId===id)));
    if(selected===id)selectScope(status?.profiles.find(p=>p.id!==id)?.id??"");
    setDeleting(undefined);setManual(undefined);setError("");setMessage(message);refresh();
  }
  const enabledProfiles=status?.profiles.filter(p=>p.enabled)??[];
  const checkingProfiles=status?.profiles.filter(p=>(status.checking_profile_ids??[status.checking_profile_id]).includes(p.id))??[];
  async function score(ids:string[]){if(busy||jobId)return;setBusy(true);setError("");try{
    const owners=ids.map(id=>viewedNodes.find(n=>n.id===id)?.profileId);
    if(!owners.length||owners.some(id=>!id))throw new Error("节点列表已变化，请刷新后重新选择");
    if(new Set(owners).size!==1)throw new Error("批量评分请一次选择同一配置的节点；也可逐个点击检测 IP 评分");
    const result=await scoreProxyNodes(owners[0]!,ids,AbortSignal.timeout(15_000));setJob(undefined);setJobId(result.job_id);
  }catch(e){setError(err(e));}finally{setBusy(false);}}
  function choose(id:string){if(!chosen.includes(id)&&chosen.length>=16){setError("每批最多选择 16 个节点");return;}setChosen(previous=>previous.includes(id)?previous.filter(x=>x!==id):[...previous,id]);}
  const nodes=useMemo(()=>viewedNodes.filter(n=>n.name.toLowerCase().includes(search.toLowerCase())||(showAll&&n.profileName.toLowerCase().includes(search.toLowerCase()))).sort((a,b)=>{
    const comparison=order==="score"?(reputationMean(b)?.value??-1)-(reputationMean(a)?.value??-1):order==="availability"?(availabilityView(b,chartMode).availability_percent??-1)-(availabilityView(a,chartMode).availability_percent??-1):a.name.localeCompare(b.name);
    return comparison||a.name.localeCompare(b.name)||a.profileName.localeCompare(b.profileName)||a.id.localeCompare(b.id);
  }),[viewedNodes,search,order,showAll,chartMode]);
  function monitoringChanged(change:MonitoringChange){
    setStatus(previous=>previous?{...previous,...(change.monitoring?{monitoring:change.monitoring}:{}),profiles:change.profile?previous.profiles.map(p=>p.id===change.profile!.id?{...p,enabled:change.profile!.enabled}:p):previous.profiles}:previous);refresh();
  }
  const controls=<ProxyMonitoring status={status}profile={profile}local={selected===LOCAL_NETWORK_PROFILE}onChanged={monitoringChanged}/>;
  const disabled=busy||Boolean(jobId)||Boolean(status?.score_busy)||status?.monitoring?.enabled===false;
  const row=(node:ViewedNode)=>{const mean=reputationMean(node);const location=nodeRegion(node);const series=availabilityView(node,chartMode);const stale=series.last_check&&Date.now()-Date.parse(series.last_check)>4*60_000;
    const checkingAt=pendingAvailabilitySlot(status?.round,series.last_check,Boolean(status?.checking&&status.monitoring?.enabled!==false&&node.profileEnabled&&!node.unsupported&&checkingProfiles.some(p=>p.id===node.profileId)&&!(chartMode==="node_link"&&series.latest?.state==="unsupported")));
    const region=FOCUS.find(r=>r.id===location.region)?.name??(location.region==="OTHER"?"其他地区":location.region);
    return <div className="proxy-lab-node" key={node.id}>
      <div className="proxy-lab-node__heading"><label><input type="checkbox" checked={chosen.includes(node.id)} disabled={disabled} onChange={()=>choose(node.id)}/><strong>{node.name}</strong><span className="proxy-node-profile">所属配置：{node.profileName}{status?.monitoring?.enabled===false?" · 总开关暂停":node.profileEnabled?"":" · 已暂停"}</span></label>
        <div className="proxy-lab-node__actions"><span className="muted">{node.protocol.toUpperCase()} · {!groupByRegion&&`${region} · `}{location.verified?"出口地区已核验":"地区按名称推断"}</span><button className="button button--small" disabled={disabled||!status?.core_available} onClick={()=>void score([node.id])}>检测 IP 评分</button><a className="button button--small button--secondary" href={`/proxy-history?profile=${encodeURIComponent(node.profileId)}&node=${encodeURIComponent(node.id)}&days=7`} target="_blank" rel="noopener noreferrer">测试历史 ↗</a><button className="button button--small button--secondary" onClick={()=>setManual(node)}>来源 / 录入</button></div>
      </div>
      <div className="proxy-lab-node__metrics"><span>{chartMode==="https"?"HTTPS 网站":"平台 → 节点"} · {series.availability_percent===null?"可用率 —":`可用率 ${series.availability_percent.toFixed(1)}%`}<small> · 覆盖 {series.coverage_percent.toFixed(1)}% · {series.observed_samples}/{series.expected_samples} 次</small></span>
        <span className={stale?"muted":series.latest?.state==="down"?"negative":""}>{stale?"历史观测 · ":""}{series.latest?.latency_ms!=null?`${series.latest.latency_ms} ms`:series.latest?.state==="down"?"本次不可用":series.latest?.state==="unsupported"?"暂不支持":series.latest?.state==="unknown"?"检测未确认":"等待检测"}</span>
        <span>IP 参考均分 {mean?`${mean.value.toFixed(0)} / 100 · ${mean.count} 个来源`:"—"}</span>
      </div>
      <ProxyAvailabilityBar history={series.history} start={node.windowStart} checkingAt={checkingAt}/>
      <div className="proxy-lab-node__times"><span>24 小时前</span><span>{series.last_check?`最近 ${formatDate(series.last_check)}`:"尚无此类历史，不补齐为正常"}</span><span>现在</span></div>
      {chartMode==="https"&&<>
        {(node.legacy_samples??0)>0&&<p className="hint">24 小时记录包含 {node.legacy_samples} 次旧规则检测，历史保留原判定；新样本使用 10 个主流来源和失败复核规则。</p>}
        <ProxyProbeResults node={node} sites={(node.latest?.policy??1)===2?status?.legacy_test_sites:(node.latest?.policy??1)===3?status?.restricted_test_sites:status?.test_sites} fallbacks={(node.latest?.policy??1)===3?status?.restricted_fallback_sites:status?.fallback_sites}/>
      </>}
      {(chartMode==="node_link"?series.latest?.detail:node.unsupported||node.latest?.detail)&&<p className="hint">{chartMode==="node_link"?series.latest?.detail:node.unsupported||node.latest?.detail}</p>}
      <details className="proxy-score-details"><summary><span className="proxy-score-details__expand">展开评分</span><span className="proxy-score-details__collapse">收起评分</span><small> · {status?.providers.length??0} 家机构</small></summary>
      <div className="proxy-source-scores">{status?.providers.map(provider=>{const value=node.scores.find(s=>s.provider===provider.id);const current=value&&currentScore(node,value);return <button key={provider.id} type="button" onClick={()=>setManual(node)} className={current?"proxy-source-score proxy-source-score--current":"proxy-source-score"}
        title={value?`${value.detail||provider.name} · ${formatDate(value.observed_at??value.checked_at)}${value.revision!==node.revision?" · 旧配置结果":""}`:provider.api?"尚未手动检测":"可打开原站核验并人工录入"}>{provider.name}<strong>{value?.purity!=null?`${value.purity.toFixed(0)}${current?"":" *"}`:value?.state==="requires_browser"?"需网页核验":value?.state==="rate_limited"?"来源限流":value?"暂无有效分":"待评分"}</strong></button>;})}</div>
      </details>
    </div>;
  };
  if(selected===LOCAL_NETWORK_PROFILE)return <Suspense fallback={<p className="hint">正在加载本机网络检测…</p>}><LocalNetworkPanel selector={<ProxyScopeSelect value={selected}profiles={status?.profiles??[]}onChange={selectScope}paused={status?.monitoring?.enabled===false}/>}controls={controls}monitoringRevision={status?.monitoring?.revision}/></Suspense>;
  const other=nodes.filter(n=>!FOCUS.some(r=>r.id===nodeRegion(n).region));
  return <section className="page-content proxy-lab" aria-label="平台代理节点分析">
    <article className="data-panel"><header className="panel-header"><div><h2>YAML 节点分析 · 平台检测</h2><p>选择本机配置或上传 YAML；每 3 分钟检测可用性，IP 评分仅手动执行。</p></div><div className="proxy-lab-node__actions"><a className="button button--small button--secondary" href={`/proxy-history?days=7${selected&&selected!==ALL_PROFILES?`&profile=${encodeURIComponent(selected)}`:""}`} target="_blank" rel="noopener noreferrer">七天测试历史 ↗</a><button className="button button--small" onClick={()=>setImporting(true)}>选择 / 上传 YAML</button></div></header>
      <div className="proxy-lab-toolbar"><ProxyScopeSelect value={selected}profiles={status?.profiles??[]}onChange={selectScope}paused={status?.monitoring?.enabled===false}/>
        <button type="button" className="button button--small button--secondary proxy-all-toggle" aria-pressed={showAll} disabled={!status?.profiles.length} onClick={toggleAll}>显示所有配置所有节点</button>
        {controls}
        {profile&&<><button className="button button--small button--secondary" onClick={()=>setRenaming(profile)}>重命名</button><button type="button" className="button button--small button--danger" onClick={()=>setDeleting(profile)}>删除配置</button></>}
        <button className="button button--small button--secondary" onClick={refresh}>刷新状态</button>
        {chosen.length>0&&<button className="button button--small" disabled={disabled||!status?.core_available} onClick={()=>void score(chosen)}>手动评分所选 {chosen.length} 个节点</button>}
      </div>
      <p className="hint">后台检测：{status?.monitoring?.enabled===false?"总开关已关闭 · ":""}{enabledProfiles.length} / {status?.profiles.length??0} 份配置已启用检测
        {status?.checking?` · 同时检测 ${checkingProfiles.length} 份配置 · 已完成 ${status.round?.completed_nodes??0}/${status.round?.expected_nodes??0} 个节点`:status?.round?.started_at?` · 本批完成 ${status.round.completed_nodes}/${status.round.expected_nodes} 个节点${status.round.duration_ms!=null?` · ${(status.round.duration_ms/1000).toFixed(1)} 秒`:""}`:" · 等待下一个三分钟检查周期"}
        {status?.round?.started_at&&status.round.saved_nodes!=null?` · 已保存 ${status.round.saved_nodes}/${status.round.expected_nodes}`:""}
        {status?.round?.timed_out_nodes?` · 超时 ${status.round.timed_out_nodes} 个（已记录未确认）`:""}
        {status?.monitoring?.enabled!==false&&status?.round?.next_run_at?` · 下次检查 ${formatDate(status.round.next_run_at)}`:""}。每 3 分钟独立异步触发全量检查，慢批次不会阻塞下一周期。请求在本周期前 50 秒内错峰启动，避免瞬时挤占；单节点同时最多 3 个网站请求，网站探测最多等待 100 秒，超时也保存结果；切换图表不触发额外检测。
      </p>
      {profile&&<p className="hint">查看：{profile.name} · {profile.source}{profile.original_filename?` · 初次记录的文件名：${profile.original_filename}`:""}</p>}
      {showAll&&<p className="hint">查看全部 {status?.profiles.length??0} 份配置 · 已读取 {viewedNodes.length} / {status?.profiles.reduce((sum,p)=>sum+p.node_count,0)??0} 个节点。支持跨配置统一筛选和排序；点击节点上的“检测 IP 评分”会使用其所属配置，批量评分一次选择同一配置。</p>}
      {failedProfiles.length>0&&<p className="form-error" role="alert">配置读取失败：{failedProfiles.map(id=>status?.profiles.find(p=>p.id===id)?.name??id).join("、")}。如有上次快照则保留显示，请刷新重试。</p>}
      <p className="hint">网站视图经各代理访问 Google、YouTube、Cloudflare、Microsoft、Apple、Bing、GitHub、Mozilla、Wikipedia、Amazon，至少 80% 可访问即通过（10 站至少 8 站）。使用 HTTPS HEAD 验证 TLS 与正确响应状态，不下载正文、不测试视频播放；每次请求最多 8 秒。同一配置内完全相同节点的不同名称共享本轮真实探测，各自保留历史。失败项经原节点独立复核，传输错误退避重试一次；限流或访问受限时最多回退一个同服务地址，遵守 Retry-After 冷却，不重复计票。限流、访问受限、网站服务错误和平台资源异常不会直接判成节点故障。节点视图独立记录平台直接连接节点端口的结果；UDP 节点不以 TCP 结果推断可用性。每个竖向大格背景汇总 15 分钟，内部从上到下堆叠 5 条横向小格，每条对应一次 3 分钟检测；悬停查看逐次状态。条纹表示该时间段观测不完整。参考均分只计当前配置、同一已查询出口 IP、24 小时内的有效来源；各机构可能使用相同底层数据。</p>
      {status&&!status.core_available&&<p className="form-error">独立 Mihomo 检测内核尚未配置；可分析 YAML，自动检测和手动 API 评分需配置内核。</p>}
      {profile?.warnings.map((warning,index)=><p className="hint" key={index}>{warning}</p>)}
      {error&&<p className="form-error" role="alert">{error}</p>}{message&&<p className="notice" role="status">{message}</p>}
      {jobId&&!job&&<p role="status">手动评分任务已创建，正在准备检测出口…</p>}
      {job&&<div className="proxy-score-progress" role="status"><strong>{job.status==="running"?"正在手动评分":job.status==="success"?"本次检测完成":job.status==="cancelled"?"检测已取消":"检测未完成"} · {job.completed}/{job.total}</strong><progress max={job.total} value={job.completed}/><p className="hint">{job.message}</p></div>}
    </article>
    {selected&&<><div className="proxy-lab-toolbar"><label>筛选节点<input value={search} onChange={e=>setSearch(e.target.value)} placeholder={showAll?"按节点或配置名称筛选":"按节点名称筛选"}/></label><label>排序<select value={order} onChange={e=>setOrder(e.target.value)}><option value="name">节点名称</option><option value="availability">24 小时可用率</option><option value="score">IP 参考均分</option></select></label>
      <label>图表内容<select aria-label="可用性图表内容" value={chartMode} onChange={e=>setChartMode(e.target.value as "https"|"node_link")}><option value="https">HTTPS 访问真实网站</option><option value="node_link">平台与节点连接</option></select></label>
      <button type="button" role="switch" aria-checked={groupByRegion} aria-label="地区分组" className="button button--small button--secondary proxy-region-toggle" onClick={()=>{setGroupByRegion(value=>!value);setShowOther(false);}}>地区分组：{groupByRegion?"开":"关"}</button>
    </div>
      <div className="proxy-availability-legend" aria-label="可用性颜色与判定标准">
        <span><i className="proxy-availability-legend__color proxy-availability-legend__color--up" aria-hidden="true"/><strong>绿色 · 正常</strong>{chartMode==="https"?"至少 8/10 站成功，成功延迟中位数 ≤ 1500 ms":"节点端口连接成功，耗时 ≤ 1500 ms"}</span>
        <span><i className="proxy-availability-legend__color proxy-availability-legend__color--slow" aria-hidden="true"/><strong>黄色 · 缓慢</strong>{chartMode==="https"?"至少 8/10 站成功，成功延迟中位数 > 1500 ms":"节点端口连接成功，耗时 > 1500 ms"}</span>
        <span><i className="proxy-availability-legend__color proxy-availability-legend__color--down" aria-hidden="true"/><strong>红色 · 不可用</strong>{chartMode==="https"?"已复核的失败足以确定通过比例低于 80%，或节点连接复核失败":"端口连接失败 / 5 秒超时；DNS 未确认时保持未知"}</span>
        <span><i className="proxy-availability-legend__color proxy-availability-legend__color--unknown" aria-hidden="true"/><strong>灰色 · 未知</strong>未采集、暂不支持或证据不足以判定，不计入可用率</span>
        <p>以上对应每次 3 分钟小格；15 分钟大格背景在出现缓慢或成功 / 失败混合时也显示黄色。</p>
      </div>
      {groupByRegion?<>
        {FOCUS.map(region=>{const items=nodes.filter(n=>nodeRegion(n).region===region.id);return <article className="data-panel proxy-region" key={region.id}><header className="panel-header"><h2>{region.flag} {region.name} <small>{items.length} 个节点</small></h2></header>{items.map(row)}{!items.length&&<p className="table-empty">此配置当前没有匹配节点。</p>}</article>;})}
        <details className="data-panel proxy-region proxy-region--other" onToggle={e=>setShowOther(e.currentTarget.open)}><summary>扩展地区 · {other.length} 个节点</summary>{showOther&&other.map(row)}{showOther&&!other.length&&<p className="table-empty">没有其他地区节点。</p>}</details>
      </>:<article className="data-panel proxy-region"><header className="panel-header"><h2>{showAll?"所有配置 · 所有节点":"全部节点"} <small>{nodes.length} 个节点</small></h2><p>{showAll?"跨配置统一排序，包含所有地区。":"所有地区合并排序，包含扩展地区。"}</p></header>{nodes.map(row)}{!nodes.length&&<p className="table-empty">{detailsLoaded?"没有匹配的节点。":"正在读取节点…"}</p>}</article>}
    </>}
    {!selected&&status&&<p className="table-empty">先选择一份本机 YAML 或上传文件，分析其节点；新历史从首次真实检测开始。</p>}
    {importing&&<ImportProfile profiles={status?.profiles??[]} onClose={()=>setImporting(false)} onImported={id=>{setSelected(id);setImporting(false);refresh();}}/>}
    {renaming&&<RenameProfile profile={renaming} onClose={()=>setRenaming(undefined)} onSaved={()=>{setRenaming(undefined);refresh();}}/>}
    {deleting&&<DeleteProfile profile={deleting} onClose={()=>setDeleting(undefined)} onDeleted={removed}/>}
    {manual&&status&&<ReputationDetails node={viewedNodes.find(n=>n.id===manual.id)??manual} providers={status.providers} onClose={()=>setManual(undefined)} onSaved={refresh}/>}
  </section>;
}

export function ProxyImportReplacement({name,profiles,checked,disabled,onChange}:{name:string;profiles:ProxyProfile[];checked:boolean;disabled:boolean;onChange:(checked:boolean)=>void}){
  const matches=sameNameProfiles(name,profiles);
  if(matches.length===0)return null;
  if(matches.length>1)return <p className="form-error">存在多份同名配置，请先重命名区分后再覆盖。</p>;
  const target=matches[0];
  return <label className="machine-confirm"><input type="checkbox"checked={checked}disabled={disabled}onChange={e=>onChange(e.target.checked)}/>覆盖同名配置“{target.name}”的内容，保留节点身份和历史</label>;
}
function ImportProfile({profiles,onClose,onImported}:{profiles:ProxyProfile[];onClose:()=>void;onImported:(id:string)=>void}){
  const dialog=useRef<HTMLDialogElement>(null);const file=useRef<HTMLInputElement>(null);const [mode,setMode]=useState("local");const [sources,setSources]=useState<LocalProxySource[]>([]);const [source,setSource]=useState("");const [name,setName]=useState("");const [customName,setCustomName]=useState(false);const [replace,setReplace]=useState(false);const [busy,setBusy]=useState(false);const [error,setError]=useState("");
  const matches=sameNameProfiles(name,profiles);const target=matches.length===1?matches[0]:undefined;
  useEffect(()=>{dialog.current?.showModal();const c=new AbortController();void fetchProxySources(c.signal).then(d=>{setSources(d.sources);if(!d.sources.length)setMode("upload");}).catch(e=>{if(!c.signal.aborted)setError(err(e));});return()=>{c.abort();if(file.current)file.current.value="";};},[]);
  useEffect(()=>{setReplace(false);},[target?.id,target?.revision]);
  async function submit(){if(busy)return;setBusy(true);setError("");try{
    if(matches.length>1)throw new Error("存在多份同名配置，请先重命名区分");
    if(target&&!replace)throw new Error("请勾选覆盖同名配置，或使用不同名称导入");
    if(replace&&!target)throw new Error("同名目标已变化，请重新选择配置");
    let content:string|undefined;
    if(mode==="upload"){const f=file.current?.files?.[0];if(!f||!f.size||f.size>1024*1024)throw new Error("请选择不超过 1 MiB 的 UTF-8 YAML 文件");content=await f.text();}else if(!source)throw new Error("请选择平台本机配置");
    const result=await importProxyProfile({name:name.trim(),...(mode==="upload"?{yaml:content,filename:file.current?.files?.[0]?.name}:{source_id:source}),...(replace&&target?{replace_id:target.id,expected_revision:target.revision}:{}),enable:true},AbortSignal.timeout(30_000));content=undefined;onImported(result.id);
  }catch(e){setError(err(e));}finally{if(file.current)file.current.value="";setBusy(false);}}
  return <dialog ref={dialog} className="credentials-dialog" onCancel={e=>{if(busy)e.preventDefault();else onClose();}}><form onSubmit={e=>{e.preventDefault();void submit();}} autoComplete="off"><div className="dialog-title"><h2>分析代理 YAML</h2><button type="button" className="icon-button" disabled={busy} onClick={onClose} aria-label="关闭">×</button></div>
    <label>配置来源<select value={mode} disabled={busy} onChange={e=>{setMode(e.target.value);setReplace(false);if(!customName)setName(e.target.value==="local"?sources.find(s=>s.id===source)?.name??"":file.current?.files?.[0]?.name??"");}}><option value="local">平台本机已有配置</option><option value="upload">上传 YAML</option></select></label>
    {mode==="local"?<label>本机配置<select value={source} disabled={busy} onChange={e=>{setSource(e.target.value);setReplace(false);if(!customName)setName(sources.find(s=>s.id===e.target.value)?.name??"");}}><option value="">请选择文件</option>{sources.map(s=><option key={s.id} value={s.id}>{s.name} · {(s.bytes/1024).toFixed(0)} KiB</option>)}</select></label>:<label>YAML 文件<input ref={file} type="file" accept=".yaml,.yml,text/yaml,application/yaml" disabled={busy} onChange={e=>{setReplace(false);if(!customName)setName(e.target.files?.[0]?.name??"");}}/></label>}
    <label>显示名称<input value={name}maxLength={128}required disabled={busy}onChange={e=>{setName(e.target.value);setCustomName(Boolean(e.target.value));setReplace(false);}}/></label>
    <ProxyImportReplacement name={name}profiles={profiles}checked={replace}disabled={busy}onChange={setReplace}/>
    <p className="hint">按显示名称匹配；不同名称作为新配置导入。同名时需明确勾选覆盖，保留原配置身份和历史。</p>
    <p className="hint">导入后按检测总开关状态加入后台检测；完全相同的内容会复用已有配置，不重复创建。</p>
    <p className="hint">仅平台执行。凭据加密保存，不回显 YAML 正文；不导入规则、DNS、TUN、控制端口或自动订阅。IP 评分仍需另外手动点击。最多 256 节点；YAML 锚点请先展开。</p>
    {error&&<p className="form-error" role="alert">{error}</p>}<div className="dialog-actions"><button type="button" className="button button--secondary" disabled={busy} onClick={onClose}>取消</button><button className="button"disabled={busy||!name.trim()||matches.length>1||Boolean(target&&!replace)}>{busy?"分析并加密保存中…":replace&&target?"确认覆盖同名配置":"分析并导入"}</button></div>
  </form></dialog>;
}

function RenameProfile({profile,onClose,onSaved}:{profile:ProxyProfile;onClose:()=>void;onSaved:()=>void}){
  const dialog=useRef<HTMLDialogElement>(null);const [name,setName]=useState(profile.name);const [busy,setBusy]=useState(false);const [error,setError]=useState("");
  useEffect(()=>{dialog.current?.showModal();},[]);
  async function save(){if(busy)return;setBusy(true);setError("");try{await renameProxyProfile(profile,name.trim());onSaved();}catch(e){setError(err(e));}finally{setBusy(false);}}
  return <dialog ref={dialog} className="credentials-dialog" onCancel={e=>{if(busy)e.preventDefault();else onClose();}}><form onSubmit={e=>{e.preventDefault();void save();}}><div className="dialog-title"><h2>重命名配置</h2><button type="button" className="icon-button" disabled={busy} onClick={onClose} aria-label="关闭">×</button></div><label>显示名称<input value={name} maxLength={128} required disabled={busy} onChange={e=>setName(e.target.value)}/></label><p className="hint">只修改显示名；节点身份、配置内容和检测历史保留。</p>{error&&<p className="form-error">{error}</p>}<div className="dialog-actions"><button type="button" className="button button--secondary" disabled={busy} onClick={onClose}>取消</button><button className="button" disabled={busy||!name.trim()}>{busy?"保存中…":"保存名称"}</button></div></form></dialog>;
}

function DeleteProfile({profile,onClose,onDeleted}:{profile:ProxyProfile;onClose:()=>void;onDeleted:(id:string,message:string)=>void}){
  const dialog=useRef<HTMLDialogElement>(null);const [confirmation,setConfirmation]=useState("");const [busy,setBusy]=useState(false);const [error,setError]=useState("");
  useEffect(()=>{dialog.current?.showModal();},[]);
  async function submit(){if(busy||confirmation.trim()!==profile.name)return;setBusy(true);setError("");try{const result=await deleteProxyProfile(profile,confirmation.trim());onDeleted(result.id,result.message);}catch(e){setError(err(e));}finally{setBusy(false);}}
  return <dialog ref={dialog} className="credentials-dialog" onCancel={e=>{if(busy)e.preventDefault();else onClose();}}><form onSubmit={e=>{e.preventDefault();void submit();}} autoComplete="off"><div className="dialog-title"><h2>删除配置</h2><button type="button" className="icon-button" disabled={busy} onClick={onClose} aria-label="关闭">×</button></div>
    <p><strong>{profile.name}</strong> · {profile.node_count} 个当前节点</p><p className="hint">{profile.source}{profile.original_filename?` · ${profile.original_filename}`:""}</p>
    <p className="form-error">将永久删除平台中的这份配置、关联节点、检测历史和 IP 评分，页面无法恢复。正在进行的检测或评分会取消，其他配置继续运行。</p>
    <p className="hint">原 YAML 文件、实际代理服务及 Agent 不受影响。若只想暂时停止检测，请取消并选择“暂停本配置检测”。</p>
    <label>输入完整配置名称确认删除<input value={confirmation} disabled={busy} required spellCheck={false} onChange={e=>setConfirmation(e.target.value)} placeholder={profile.name}/></label>
    {error&&<p className="form-error" role="alert">{error}</p>}<div className="dialog-actions"><button type="button" className="button button--secondary" disabled={busy} onClick={onClose}>取消</button><button className="button button--danger" type="submit" disabled={busy||confirmation.trim()!==profile.name}>{busy?"正在删除…":"永久删除此配置"}</button></div>
  </form></dialog>;
}

export function ReputationResults({node,providers}:{node:ProxyLabNode;providers:ReputationProvider[]}){
  return <div className="table-scroll"><table><thead><tr><th>机构</th><th>纯净度 / 原分</th><th>状态与时间</th><th>原站</th></tr></thead><tbody>{providers.map(p=>{const s=node.scores.find(s=>s.provider===p.id);return <tr key={p.id}><td>{p.name}<small>{p.api?"公开 API":"网页核验 / 人工录入"}</small></td><td>{s?.purity!=null?`${s.purity.toFixed(0)} / 100`:"—"}{s?.raw_score!=null&&<small>原分 {s.raw_score} · {s.direction==="risk"?"高分高风险":"高分更纯净"}</small>}</td><td>{s?<><span>{s.method==="manual"?"人工录入":s.state==="ok"?"已取得":s.state}</span><small>{formatDate(s.observed_at??s.checked_at)}</small><small>{s.detail}</small>{!currentScore(node,s)&&<small className="muted">未进入当前参考均值</small>}</>:"尚未手动检测"}</td><td><a className="button button--small button--secondary" href={p.url} target="_blank" rel="noopener noreferrer">打开原站 ↗</a></td></tr>;})}</tbody></table></div>;
}
function ReputationDetails({node,providers,onClose,onSaved}:{node:ProxyLabNode;providers:ReputationProvider[];onClose:()=>void;onSaved:()=>void}){
  const dialog=useRef<HTMLDialogElement>(null);const [provider,setProvider]=useState(providers[0]?.id??"");const [ip,setIp]=useState(nodeEgress(node)??"");const [raw,setRaw]=useState("");const [direction,setDirection]=useState(providers[0]?.direction??"unknown");const [at,setAt]=useState(()=>{const date=new Date();date.setMinutes(date.getMinutes()-date.getTimezoneOffset());return date.toISOString().slice(0,16);});const [busy,setBusy]=useState(false);const [error,setError]=useState("");const [message,setMessage]=useState("");
  useEffect(()=>{dialog.current?.showModal();},[]);
  async function save(){if(busy)return;setBusy(true);setError("");try{if(!raw||direction==="unknown")throw new Error("请填写原分并确认评分方向");setMessage((await saveManualReputation(node,provider,{ip:ip.trim(),raw_score:Number(raw),direction,observed_at:new Date(at).toISOString()})).message);onSaved();}catch(e){setError(err(e));}finally{setBusy(false);}}
  return <dialog ref={dialog} className="credentials-dialog proxy-reputation-dialog" onCancel={e=>{if(busy)e.preventDefault();else onClose();}}><div className="dialog-title"><h2>多机构评分 · {node.name}</h2><button className="icon-button" disabled={busy} onClick={onClose} aria-label="关闭">×</button></div>
    <p>参考出口 IP：<code>{nodeEgress(node)??"尚未取得"}</code></p><p className="hint">以平台检测出口为先，仅有人工结果时使用录入 IP。来源限流、验证或证据不足时保留原因，不补零分。原站链接在你的浏览器打开；请查询这里显示的出口 IP，避免误查浏览器自身出口。</p>
    <ReputationResults node={node} providers={providers}/><form onSubmit={e=>{e.preventDefault();void save();}} className="proxy-manual-score"><h3>录入原站评分</h3>
      <label>机构<select value={provider} onChange={e=>{setProvider(e.target.value);setDirection(providers.find(p=>p.id===e.target.value)?.direction??"unknown");}}>{providers.map(p=><option key={p.id} value={p.id}>{p.name}</option>)}</select></label>
      <label>查询的出口 IP<input value={ip} required onChange={e=>setIp(e.target.value)}/></label><label>原始分值<input type="number" min={0} max={100} step="any" required value={raw} onChange={e=>setRaw(e.target.value)}/></label>
      <label>原站分值方向<select value={direction} required onChange={e=>setDirection(e.target.value)}><option value="unknown">请确认方向</option><option value="risk">分数越高风险越高</option><option value="purity">分数越高越纯净</option></select></label><label>原站查询时间<input type="datetime-local" required value={at} onChange={e=>setAt(e.target.value)}/></label>
      <p className="hint">人工录入会单独标注，不会伪装为机构 API 返回。</p>{error&&<p className="form-error">{error}</p>}{message&&<p className="notice">{message}</p>}<div className="dialog-actions"><button type="button" className="button button--secondary" disabled={busy} onClick={onClose}>关闭</button><button className="button" disabled={busy}>{busy?"保存中…":"保存人工评分"}</button></div>
    </form></dialog>;
}
