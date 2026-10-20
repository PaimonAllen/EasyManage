import { useEffect,useRef,useState } from "react";
import { fetchPlatformNetwork,type PlatformNetworkStatus } from "../api/client";
import { NETWORK_LABELS,networkRates } from "../lib/networkActivity";
import { formatBytes,formatDate,formatRate } from "../lib/format";
import type { NetworkActivity,NetworkGroupMetric } from "../types/monitoring";

function count(value:number){return value.toLocaleString("zh-CN");}
function hasByteAccounting(row:NetworkGroupMetric){return row.group!=="ssh_connect"&&row.group!=="tcp_probe";}
export function NetworkResourceRows({name,activity,previous,online=true}: {
  name:string;activity:NetworkActivity|null;previous?:NetworkActivity|null;online?:boolean;
}){
  if(!activity)return <tr><td><strong>{name}</strong></td><td colSpan={7}>未上报网络统计，需更新 Agent；不按 0 处理。</td></tr>;
  return <>{activity.groups.map(row=>{
    const rate=networkRates(activity,previous,row);
    const bytes=hasByteAccounting(row);
    return <tr key={row.group}>
      <td><strong>{name}</strong><small>PID {activity.process_id} · {online?"当前进程":"离线历史"}</small><small>统计起点 {formatDate(activity.started_at)}</small></td>
      <td><strong>{NETWORK_LABELS[row.group]}</strong><small>{row.group==="ssh_tunnel"?"内层 TLS 字节；不与 HTTP 相加":row.group==="ssh_connect"?"真实新 TCP / SSH 握手；通道复用不加次数":row.group==="tcp_probe"?"只探测端口，不算 SSH 登录":row.group.startsWith("agent_")?"发送为尝试提交的 JSON 正文预算":"应用层正文 / 通道数据，不含协议头"}</small></td>
      <td className="number">{bytes?formatBytes(row.received_bytes):"—"}<small>{bytes&&online&&rate?formatRate(rate.received):"—"}</small></td>
      <td className="number">{bytes?formatBytes(row.submitted_bytes):"—"}<small>{bytes&&online&&rate?formatRate(rate.submitted):"—"}</small></td>
      <td className="number"><strong>{count(row.started)}</strong><small>已结束 {count(row.completed)}{online&&rate?` · ${Number(rate.requests.toPrecision(3))}/s`:""}</small></td>
      <td className={row.failed?"number form-error":"number"}>{count(row.failed)}</td>
      <td className="number">{online?count(row.in_flight):"—"}<small>持有连接 {online&&row.open_connections!=null?count(row.open_connections):"—"}</small></td>
      <td>{formatDate(activity.collected_at)}</td>
    </tr>;
  })}</>;
}

export function NetworkResources(){
  const [data,setData]=useState<PlatformNetworkStatus|null>(null);
  const [previous,setPrevious]=useState<PlatformNetworkStatus|null>(null);
  const [error,setError]=useState("");const [supported,setSupported]=useState(true);
  const last=useRef<PlatformNetworkStatus|null>(null);
  useEffect(()=>{
    const controller=new AbortController();let pending=false;
    async function refresh(){
      if(pending||document.hidden)return;pending=true;
      try{
        const result=await fetchPlatformNetwork(AbortSignal.any([controller.signal,AbortSignal.timeout(10_000)]));
        if(controller.signal.aborted)return;
        setSupported(result!==null);setPrevious(last.current);setData(result);last.current=result;setError("");
      }catch(error){if(!controller.signal.aborted)setError(error instanceof Error?error.message:"网络资源统计读取失败");}
      finally{pending=false;}
    }
    void refresh();const timer=window.setInterval(()=>void refresh(),15_000);
    document.addEventListener("visibilitychange",refresh);
    return()=>{controller.abort();window.clearInterval(timer);document.removeEventListener("visibilitychange",refresh);};
  },[]);
  return <article className="data-panel" id="platform-network-resources">
    <header className="panel-header"><div><h2>网络资源分析 · 本平台与 Agent</h2><p>本次进程累计 · 每 15 秒刷新 · 相邻真实采样计算速率，重启清零；不是过去 24 小时统计</p></div></header>
    {error&&<p className="form-error" role="alert">{error}</p>}
    {!supported&&<p className="table-empty">网络资源分析接口尚不可用，需要更新平台后端。</p>}
    {supported&&!data&&!error&&<p className="table-empty">正在读取网络资源统计…</p>}
    {data&&<><div className="table-scroll"><table className="network-resource-table"><thead><tr><th>机器 / 组件</th><th>链路 / 口径</th><th>接收正文累计 / 速率</th><th>提交正文累计 / 速率</th><th>请求 / 通道发起数</th><th>失败 / 取消</th><th>进行中 / 连接</th><th>采样时间</th></tr></thead><tbody>
      <NetworkResourceRows name="EasyManage 平台" activity={data.server} previous={previous?.server}/>
      {data.agents.map(agent=><NetworkResourceRows key={agent.node_id} name={`${agent.hostname} · Agent`} activity={agent.activity}
        previous={previous?.agents.find(item=>item.node_id===agent.node_id)?.activity} online={!agent.monitoring_paused&&Date.now()-Date.parse(agent.last_seen)<=45_000}/>) }
    </tbody></table></div>{!data.agents_available&&<p className="form-error">节点存储暂不可用，只展示平台本次进程的本地计数。</p>}</>}
    <p className="hint">仅记录本项目固定分类的次数和字节总数，不记录 URL、请求头、密码、私钥、终端或业务内容，不抓包、不增加 SSH 连接。本页轮询本身也计入平台开销。Agent 的“提交正文”包含失败及重试时提交给 HTTP 客户端的正文预算，不证明已在线路发送；平台接收只计实际消费的正文。SSH 命令/上传计通道读写数据，不含命令协议、被忽略的 stderr、握手及保活字节。SSH 新连接失败数仅覆盖 TCP/握手，非账户认证结果。</p>
    <p className="hint">同一上报在 Agent、平台接收和 SSH 隧道中分别可见，不可相加；WebSocket 和 SSH 终端也属不同层。除隧道内层 TLS 字节外，不含 TLS/SSH/TCP/IP 头、重传、网卡整机流量及 PostgreSQL 线路字节，也不包含外部部署脚本等子进程的流量。SSH 由平台发起，Agent 不发起 SSH。HTTP 底层连接池数量未统计，显示“—”。首次采样、进程重启、计数回退或离线不伪造速率；旧 Agent 显示未知而不是 0。</p>
  </article>;
}
