import {useEffect,useMemo,useState,type ReactNode} from "react";
import {fetchLocalNetwork,type LocalNetworkData,type LocalNetworkSite} from "../api/proxyLab";
import {ProxyAvailabilityBar} from "./ProxyAnalysis";
import {localHttpStatus,localNetworkState,localHttpFallback} from "../lib/localNetwork";
import {pendingAvailabilitySlot} from "../lib/proxyAvailability";
import {formatDate} from "../lib/format";
const errorMessage=(e:unknown)=>e instanceof Error?e.message:"本机网络检测暂不可用";
export function LocalWebsiteCard({site,start,checkingAt=null}:{site:LocalNetworkSite;start:string;checkingAt?:string|null}){
  const latest=site.latest;const limited=latest?.http_status!=null&&latest.http_status>=400;
  return <article className="proxy-lab-node"><div className="proxy-lab-node__heading"><strong><a href={site.url}target="_blank"rel="noopener noreferrer">{site.name} ↗</a></strong><div className="proxy-lab-node__actions"><span className="muted">{new URL(site.url).hostname} · 本机直连 HTTPS</span><a className="button button--small button--secondary" href={`/proxy-history?source=local&node=${encodeURIComponent(site.id)}&days=7`}target="_blank"rel="noopener noreferrer">测试历史 ↗</a></div></div>
    <div className="proxy-lab-node__metrics"><span>网络可用率 {site.availability_percent==null?"—":`${site.availability_percent.toFixed(1)}%`}<small> · 覆盖 {site.coverage_percent.toFixed(1)}% · {site.observed_samples}/{site.expected_samples} 次</small></span><span className={latest?.state==="down"?"negative":""}>{localNetworkState(latest?.state)}{latest?.latency_ms!=null?` · ${latest.latency_ms} ms`:""}</span><span className={limited?"local-network__http-warning":"muted"}>{localHttpStatus(latest?.http_status)}</span></div>
    <ProxyAvailabilityBar history={site.history}start={start}checkingAt={checkingAt}/><div className="proxy-lab-node__times"><span>24 小时前</span><span>{site.last_check?`最近 ${formatDate(site.last_check)}`:"尚无检测记录"}</span><span>现在</span></div>
    {latest?.http_fallback&&<p className="hint">{localHttpFallback(latest.http_fallback)}</p>}{latest?.detail&&<p className="hint">{latest.detail}</p>}
  </article>;
}
export default function LocalNetworkPanel({selector,controls,monitoringRevision}:{selector:ReactNode;controls?:ReactNode;monitoringRevision?:number}){
  const [data,setData]=useState<LocalNetworkData>();const [error,setError]=useState("");const [revision,setRevision]=useState(0);
  const [search,setSearch]=useState("");const [order,setOrder]=useState("name");
  useEffect(()=>{
    const controller=new AbortController();let pending=false;
    const load=async()=>{if(pending||document.hidden)return;pending=true;try{const next=await fetchLocalNetwork(AbortSignal.any([controller.signal,AbortSignal.timeout(15_000)]));if(!controller.signal.aborted){setData(next);setError("");}}catch(e){if(!controller.signal.aborted)setError(errorMessage(e));}finally{pending=false;}};
    void load();const timer=window.setInterval(()=>void load(),30_000);document.addEventListener("visibilitychange",load);
    return()=>{controller.abort();window.clearInterval(timer);document.removeEventListener("visibilitychange",load);};
  },[revision,monitoringRevision]);
  const sites=useMemo(()=>data?.nodes.filter(s=>(s.name+" "+s.url).toLowerCase().includes(search.toLowerCase())).sort((a,b)=>order==="availability"?(b.availability_percent??-1)-(a.availability_percent??-1):order==="latency"?(a.latest?.latency_ms??Infinity)-(b.latest?.latency_ms??Infinity):a.name.localeCompare(b.name)),[data,search,order]);
  const round=data?.round;
  return <section className="page-content proxy-lab" aria-label="本机网络检测"><article className="data-panel"><header className="panel-header"><div><h2>本机网络检测 · 平台出口</h2><p>每 3 分钟直连国内 20 个网站 · 状态每 30 秒刷新 · 七天历史</p></div><a className="button button--small button--secondary"href="/proxy-history?source=local&days=7"target="_blank"rel="noopener noreferrer">七天测试历史 ↗</a></header>
    <div className="proxy-lab-toolbar">{selector}{controls}<button className="button button--small button--secondary"onClick={()=>setRevision(n=>n+1)}>刷新状态</button></div>
    <p className="hint">使用平台本机网络直接发起 HTTPS HEAD，验证 DNS、连接及 TLS；HEAD 返回 404、405、501 时，额外用一次 GET 兼容复核，只读取响应头，保留两次状态。不使用已导入的代理节点，遵循本机系统路由，不主动读取或保存正文、不跟随重定向。收到有效 HTTPS/HTTP 响应表示网络可达；403、429 等真实限制仍单独标注。图表反映网络连通性，不代表网站所有业务正常。</p>
    <p className="hint">后台每 3 分钟独立检测全部 20 个网站，每次请求最多 8 秒、单站最多 18 秒；HEAD 连接失败最多重试一次，GET 兼容复核不重试。页面刷新只读取数据库结果。
      {round?.started_at?` · 已完成 ${round.completed_sites}/${round.expected_sites} · 已保存 ${round.saved_sites}/${round.expected_sites}`:""}{round?.duration_ms!=null?` · ${(round.duration_ms/1000).toFixed(1)} 秒`:""}{data?.enabled&&round?.next_run_at?` · 下次 ${formatDate(round.next_run_at)}`:""}</p>
    {data&&!data.enabled&&<p className="hint local-network__http-warning">本机网络检测已暂停，当前仅展示保留的历史记录。</p>}
    {(error||round?.error)&&<p className="form-error"role="alert">{error||round?.error}</p>}
  </article><div className="proxy-lab-toolbar"><label>筛选网站<input value={search}onChange={e=>setSearch(e.target.value)}placeholder="按网站名称或域名筛选"/></label><label>排序<select value={order}onChange={e=>setOrder(e.target.value)}><option value="name">网站名称</option><option value="availability">24 小时网络可用率</option><option value="latency">最近响应耗时</option></select></label></div>
  <article className="data-panel"><header className="panel-header"><h2>网站稳定性 <small>{sites?.length??0} 个网站</small></h2></header>{sites?.map(site=><LocalWebsiteCard key={site.id}site={site}start={data!.window_start}checkingAt={pendingAvailabilitySlot(round,site.last_check,Boolean(data?.enabled))}/>)}{!data&&<p className="table-empty">正在读取网站检测记录…</p>}{data&&!sites?.length&&<p className="table-empty">没有匹配的网站。</p>}</article></section>;
}
