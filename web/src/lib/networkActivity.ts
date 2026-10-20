import type { NetworkActivity,NetworkGroup,NetworkGroupMetric } from "../types/monitoring";

export const NETWORK_LABELS:Record<NetworkGroup,string>={
  http_browser:"平台接收 · 页面 / API",http_agent:"平台接收 · Agent HTTP",
  http_service:"平台发起 · Gitea HTTP",http_probe:"平台发起 · HTTPS 自检",
  agent_enroll:"Agent 发起 · 注册",agent_report:"Agent 发起 · 完整上报",agent_live:"Agent 发起 · 实时指标",agent_control:"Agent 发起 · 轻量控制",
  ssh_connect:"SSH 新连接 / 握手",ssh_exec:"SSH 命令 / 终端通道",ssh_scp:"SSH 文件上传通道",
  ssh_tunnel:"SSH 隧道承载 HTTPS",tcp_probe:"SSH 端口 TCP 探测（非登录）",web_socket:"浏览器 WebSocket 终端",
};

export function networkRates(current:NetworkActivity,previous:NetworkActivity|null|undefined,row:NetworkGroupMetric):{received:number;submitted:number;requests:number}|null{
  if(!previous||previous.process_id!==current.process_id||previous.started_at!==current.started_at)return null;
  const before=previous.groups.find(item=>item.group===row.group);
  const seconds=(Date.parse(current.collected_at)-Date.parse(previous.collected_at))/1000;
  if(!before||!Number.isFinite(seconds)||seconds<=0||row.received_bytes<before.received_bytes||row.submitted_bytes<before.submitted_bytes||row.started<before.started)return null;
  return {received:(row.received_bytes-before.received_bytes)/seconds,submitted:(row.submitted_bytes-before.submitted_bytes)/seconds,requests:(row.started-before.started)/seconds};
}
