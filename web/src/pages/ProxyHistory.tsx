import {useEffect,useRef,useState} from "react";
import {fetchProxyLab,fetchProxyHistory,fetchProxyHistoryLogs,fetchLocalNetworkHistory,fetchLocalNetworkLogs,LOCAL_NETWORK_PROFILE,type ProxyLabStatus,type ProxyHistoryData,type ProxyHistoryLogs,type ProxyHistoryEntry,type ProxyAvailabilitySeries} from "../api/proxyLab";
import {ProxyAvailabilityBar,ProxyProbeResults} from "../components/ProxyAnalysis";
import {localHttpStatus,localNetworkState,localHttpFallback} from "../lib/localNetwork";
import {formatDate} from "../lib/format";
import "./ProxyHistory.css";
const validId=(value:string)=>/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value);
const message=(value:unknown)=>value instanceof Error?value.message:"测试历史暂不可用";
const stateName=(state:string|undefined)=>({up:"正常",slow:"缓慢",down:"未通过",unknown:"未确认",unsupported:"不适用"}[state??""]??"未采集");
const stateClass=(state:string|undefined)=>state==="down"?"negative":state==="up"?"positive":"muted";
export function ProxyHistoryCharts({series,start,days,mode,local=false}:{series:ProxyAvailabilitySeries;start:string;days:number;mode:"https"|"node_link";local?:boolean}){
  return <section className="data-panel proxy-history__charts"><h2>{days===7?"七天":"24 小时"} · {local?"本机网络 HTTPS 连通性":mode==="https"?"代理 HTTPS 网站访问":"平台到节点端口"}</h2>
    <p className="hint">每行 24 小时，每条小格 3 分钟；灰色表示未采集、未确认或不适用。历史保留当时规则和真实缺口。</p>
    {Array.from({length:days},(_,index)=>{
      const at=new Date(Date.parse(start)+index*86400_000).toISOString();const history=series.history.slice(index*480,(index+1)*480);
      const up=[...history].filter(c=>c==="u"||c==="s").length,known=[...history].filter(c=>"usd".includes(c)).length;
      return <div className="proxy-history__day" key={at}><div><strong>{formatDate(at)}</strong><span>可用率 {known?`${(up/known*100).toFixed(1)}%`:"—"} · 有效 {known}/480 次</span></div><ProxyAvailabilityBar history={history} start={at} label={`${formatDate(at)} 起 24 小时可用性`}/></div>;
    })}
  </section>;
}
export function ProxyHistoryLogEntry({entry,mode,status,local=false}:{entry:ProxyHistoryEntry;mode:"https"|"node_link";status?:ProxyLabStatus;local?:boolean}){
  const o=entry.observation;const https=local||mode==="https";const result=https?o:o.link;
  const sites=o.policy===2?status?.legacy_test_sites:o.policy===3?status?.restricted_test_sites:status?.test_sites;
  const fallbacks=o.policy===3?status?.restricted_fallback_sites:status?.fallback_sites;
  return <details className="proxy-history__entry"><summary><time>{formatDate(entry.checked_at)}</time>
    <span className={stateClass(result?.state)}>{local?"本机 → 网站":https?"HTTPS":"平台 → 节点"} · {local?localNetworkState(result?.state):stateName(result?.state)}{result?.latency_ms!=null?` · ${result.latency_ms} ms`:""}</span>
  </summary><div><p className="hint">时间槽 {formatDate(entry.slot)}{local?" · 平台本机直连":` · 配置版本 ${entry.revision}`}
    {local?"":https?` · 检测规则 ${o.policy??1} · ${o.sites?.length?"HTTPS 通过对应代理节点访问":"该轮未取得逐站 HTTPS 结果"}`:` · ${o.link?.kind?.toUpperCase()??"节点端口"} 测试`}</p>
    {local?<><p className={(o.http_status??0)>=400?"local-network__http-warning":"hint"}>{localHttpStatus(o.http_status)}</p>{o.http_fallback&&<p className="hint">{localHttpFallback(o.http_fallback)}</p>}{o.detail&&<p>{o.detail}</p>}</>:https?<>{o.detail&&<p>{o.detail}</p>}<ProxyProbeResults node={{latest:o}}sites={sites}fallbacks={fallbacks}defaultOpen/></>
      :<p>节点端口：{o.link?.detail??"该轮未采集节点连接数据"}{o.link?.latency_ms!=null?` · ${o.link.latency_ms} ms`:""}</p>}
  </div></details>;
}
export default function ProxyHistory(){
  const [initial]=useState(()=>new URLSearchParams(typeof window==="undefined"?"":window.location.search));
  const [profile,setProfile]=useState(initial.get("source")==="local"?LOCAL_NETWORK_PROFILE:initial.get("profile")??"");const [node,setNode]=useState(initial.get("node")??"");
  const [days,setDays]=useState(initial.get("days")==="1"?1:7);const [mode,setMode]=useState<"https"|"node_link">("https");
  const [status,setStatus]=useState<ProxyLabStatus>();const [data,setData]=useState<ProxyHistoryData>();const [logs,setLogs]=useState<ProxyHistoryLogs>();
  const [error,setError]=useState("");const [logError,setLogError]=useState("");const [busy,setBusy]=useState(false);const [logBusy,setLogBusy]=useState(false);const [revision,setRevision]=useState(0);
  const logAbort=useRef<AbortController|null>(null);
  const [theme,setTheme]=useState(()=>{try{return localStorage.getItem("easymanage-theme")==="light"?"light":"dark";}catch{return "dark";}});
  useEffect(()=>{document.documentElement.dataset.theme=theme;},[theme]);
  useEffect(()=>{
    const controller=new AbortController();const timer=setTimeout(()=>{void fetchProxyLab(AbortSignal.any([controller.signal,AbortSignal.timeout(15_000)])).then(value=>{
      if(controller.signal.aborted)return;setStatus(value);setProfile(current=>current||value.profiles[0]?.id||"");
    }).catch(e=>{if(!controller.signal.aborted)setError(message(e));});},0);
    return()=>{clearTimeout(timer);controller.abort();};
  },[]);
  useEffect(()=>{
    if(!profile){setBusy(false);return;}
    if((profile!==LOCAL_NETWORK_PROFILE&&!validId(profile))||(node&&!validId(node))){setBusy(false);setError("配置或节点 ID 无效，请从 Global Network 打开测试历史。");return;}
    const controller=new AbortController();const timer=setTimeout(()=>{setBusy(true);setError("");setData(undefined);setLogs(undefined);
      const signal=AbortSignal.any([controller.signal,AbortSignal.timeout(25_000)]);
      void (profile===LOCAL_NETWORK_PROFILE?fetchLocalNetworkHistory(node,days,signal):fetchProxyHistory(profile,node,days,signal)).then(value=>{
        if(!controller.signal.aborted){setData(value);if(value.selected_node)window.history.replaceState(null,"",`/proxy-history?${new URLSearchParams({...((value.source==="local_network")?{source:"local"}:{profile:value.profile.id}),node:value.selected_node.id,days:String(value.days)})}`);document.title=`${value.selected_node?.name??value.profile.name} · 测试历史`;}
      }).catch(e=>{if(!controller.signal.aborted)setError(message(e));}).finally(()=>{if(!controller.signal.aborted)setBusy(false);});
    },0);return()=>{clearTimeout(timer);controller.abort();};
  },[profile,node,days,revision]);
  useEffect(()=>{
    const controller=new AbortController();logAbort.current=controller;setLogs(undefined);setLogError("");setLogBusy(false);
    if(!data?.selected_node)return()=>controller.abort();
    const timer=setTimeout(()=>{setLogBusy(true);void (data.source==="local_network"?fetchLocalNetworkLogs(data.selected_node!.id,data.days,null,controller.signal):fetchProxyHistoryLogs(data.profile.id,data.selected_node!.id,data.days,null,controller.signal)).then(value=>{
      if(!controller.signal.aborted)setLogs(value);
    }).catch(e=>{if(!controller.signal.aborted)setLogError(message(e));}).finally(()=>{if(!controller.signal.aborted)setLogBusy(false);});},0);
    return()=>{clearTimeout(timer);controller.abort();};
  },[data]);
  async function more(){
    const controller=logAbort.current;if(!controller||controller.signal.aborted||!data?.selected_node||!logs?.next_before||logBusy)return;
    setLogBusy(true);setLogError("");try{
      const value=await (data.source==="local_network"?fetchLocalNetworkLogs(data.selected_node.id,data.days,logs.next_before,controller.signal):fetchProxyHistoryLogs(data.profile.id,data.selected_node.id,data.days,logs.next_before,controller.signal));
      if(!controller.signal.aborted)setLogs(previous=>({...value,entries:[...(previous?.entries??[]),...value.entries]}));
    }catch(e){if(!controller.signal.aborted)setLogError(message(e));}finally{if(!controller.signal.aborted)setLogBusy(false);}
  }
  const local=profile===LOCAL_NETWORK_PROFILE;
  const series=data?.[mode];
  return <div className="dashboard proxy-history"><header className="header"><a className="brand" href="/#/global-network">EasyManage</a><button className="theme-toggle" onClick={()=>setTheme(value=>value==="light"?"dark":"light")}>{theme==="light"?"深色":"浅色"}</button></header>
    <main><header className="proxy-history__heading"><div><h1>{local?"本机网络 · 测试历史":"代理节点 · 测试历史"}</h1><p>数据库已保存结果 · 最长保留 7 天 · 按需加载，不启动额外测试</p></div><button className="button button--secondary" disabled={busy||logBusy} onClick={()=>setRevision(value=>value+1)}>重新读取</button></header>
      <div className="proxy-history__filters"><label>检测来源<select value={profile} onChange={e=>{setData(undefined);setLogs(undefined);setProfile(e.target.value);setNode("");if(e.target.value===LOCAL_NETWORK_PROFILE)setMode("https");}}><option value="">请选择配置</option><option value={LOCAL_NETWORK_PROFILE}>本机网络检测 · 20 个网站</option>{status?.profiles.map(p=><option key={p.id}value={p.id}>{p.name}{p.enabled?"":" · 已暂停"}</option>)}</select></label>
        <label>{local?"网站":"节点"}<select value={node||data?.selected_node?.id||""}disabled={!data}onChange={e=>setNode(e.target.value)}><option value="">请选择{local?"网站":"节点"}</option>{data?.nodes.map(n=><option key={n.id}value={n.id}>{n.name}</option>)}{data?.selected_node&&!data.nodes.some(n=>n.id===data.selected_node!.id)&&<option value={data.selected_node.id}>{data.selected_node.name} · 历史节点</option>}</select></label>
        <label>查看范围<select value={days}onChange={e=>setDays(Number(e.target.value))}><option value={7}>七天视图</option><option value={1}>24 小时视图</option></select></label>
        {!local&&<label>测试类型<select value={mode}onChange={e=>setMode(e.target.value as "https"|"node_link")}><option value="https">HTTPS 网站访问</option><option value="node_link">平台到节点连通性</option></select></label>}
      </div>
      <p className="notice">{local?"本机直连国内网站，收到有效 HTTPS/HTTP 响应表示网络可达；403、429 等网站限制和 5xx 服务异常单独标注。":mode==="https"?"新规则为 10 个来源至少 8 个通过。HTTPS 网站请求经对应代理节点发送。":"此视图展示平台直接连接节点端口的结果与耗时；UDP 专用节点明确标注 TCP 检测不适用。"}旧规则历史保留原判定，未采集的时间不会补造。</p>
      {error&&<p className="form-error" role="alert">{error}</p>}{busy&&<p role="status">正在读取历史记录…</p>}
      {data&&<><p className="hint">{data.profile.name} · {data.selected_node?.name??"暂无节点"} · 读取于 {formatDate(data.generated_at)}。仅复用现有精简轮次记录；不保存网站正文、代理凭据或额外日志副本。</p>
        {series&&<><div className="proxy-history__summary"><span>可用率 <strong>{series.availability_percent==null?"—":`${series.availability_percent.toFixed(1)}%`}</strong></span><span>有效样本 <strong>{series.observed_samples}/{series.expected_samples}</strong></span><span>观测覆盖 <strong>{series.coverage_percent.toFixed(1)}%</strong></span><span>最近结果 <strong className={stateClass(series.latest?.state)}>{local?localNetworkState(series.latest?.state):stateName(series.latest?.state)}</strong></span></div><ProxyHistoryCharts series={series}start={data.window_start}days={data.days}mode={mode}local={local}/></>}
        <section className="data-panel proxy-history__logs"><header><h2>{local?"本机网络测试日志":mode==="https"?"HTTPS 网站测试日志":"平台到节点连通性日志"}</h2><p className="hint">每页 50 轮，最多 7 天；{local?"本机直连网站的 HTTP 状态、耗时和失败原因":mode==="https"?"逐站耗时、失败、限流和冷却信息":"节点端口状态、连接耗时和失败原因"}均来自当时的数据库记录。</p></header>
          {logError&&<p className="form-error" role="alert">{logError}</p>}
          {logs?.entries.map(entry=><ProxyHistoryLogEntry key={`${mode}:${entry.slot}`}entry={entry}mode={mode}status={status}local={local}/>)}
          {logBusy&&<p role="status">正在读取日志…</p>}{!logBusy&&!logError&&logs&&!logs.entries.length&&<p className="table-empty">所选时段没有已保存记录；历史会从实际检测开始累积。</p>}
          {logs?.has_more&&<button className="button button--secondary" disabled={logBusy}onClick={()=>void more()}>加载更早的 50 轮</button>}
        </section></>}
    </main>
  </div>;
}
