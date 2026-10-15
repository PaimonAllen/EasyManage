import { useMemo, useState } from "react";
import { CollectionNotes, NodeStatus, SnapshotAge } from "../components/NodeStatus";
import { PageHeading } from "../components/PageHeading";
import { analyzeProxyPorts } from "../lib/proxyPorts";
import type { NodeSummary, ProxyService } from "../types/monitoring";

import { inventoryFresh } from "../lib/collection";

function listenerScope(addresses: string[]) {
  if (!addresses.length) return "未知 / 无权限读取";
  if (addresses.some((address) => /:\/\/(?:0\.0\.0\.0|\[::\]):/.test(address))) return "所有接口";
  if (addresses.some((address) => !/:\/\/(?:127\.|\[::1\]:)/.test(address))) return "指定地址";
  return "仅本机";
}

interface ProxyRow {
  node: NodeSummary;
  service: ProxyService;
}

export function GlobalNetwork({ nodes }: { nodes: NodeSummary[] }) {
  const [selectedNode, setSelectedNode] = useState("");
  const visibleNodes = nodes.filter((node) => !selectedNode || node.id === selectedNode);
  const rows = useMemo<ProxyRow[]>(() => nodes
    .filter((node) => !selectedNode || node.id === selectedNode)
    .flatMap((node) => (node.metrics?.proxy_services ?? []).map((service) => ({ node, service })))
    .sort((left, right) => left.node.hostname.localeCompare(right.node.hostname)
      || left.service.username.localeCompare(right.service.username)
      || left.service.name.localeCompare(right.service.name)), [nodes, selectedNode]);
  const current = rows.filter(({ node }) => inventoryFresh(node));
  const users = new Set(current.map(({ node, service }) => `${node.id}:${service.uid}`));
  const visibleListeners = current.reduce((count, { service }) => count + service.listen_addresses.length, 0);
  const portAnalysis = analyzeProxyPorts(current.map(({ node, service }) => ({
    nodeId: node.id, hostname: node.hostname, service,
  })));
  const incomplete = visibleNodes.some((node) => !inventoryFresh(node)
    || node.metrics?.privilege?.mode !== "full"
    || node.metrics?.collection_issues?.some((issue) =>
      (issue.component === "proxy" || issue.component === "processes")
      && /部分|不可读取|不完整|上限|未知/.test(issue.message)));
  const portStatus = portAnalysis.conflicts.length
    ? { tone: "conflict", label: `疑似冲突 ${portAnalysis.conflicts.length} 组` }
    : portAnalysis.dualStackUncertain
      ? { tone: "uncertain", label: "双栈端口待确认" }
      : incomplete || portAnalysis.invalidAddresses
        ? { tone: "uncertain", label: "部分监听待确认" }
        : portAnalysis.observedListeners
          ? { tone: "healthy", label: "未发现可见冲突" }
          : { tone: "unknown", label: "无可见监听" };

  return <>
    <PageHeading eyebrow="Proxy observability" title="Global Network · 代理" description="按服务器和用户查看 Clash / Mihomo、sing-box、V2Ray 等代理服务的只读运行观测" />
    <section className="page-content">
      <article className="data-panel">
        <header className="panel-header"><div><h2>代理运行概览</h2><p>只统计新鲜 Agent 清单；不将历史快照算作当前运行。</p></div></header>
        <div className="health-grid">
          <div><strong>代理进程</strong><span>{current.length}</span><small>可识别、可见的进程</small></div>
          <div><strong>涉及用户</strong><span>{users.size}</span><small>按节点和 UID 去重</small></div>
          <div><strong>可见监听</strong><span>{visibleListeners}</span><small>地址不等于代理可用</small></div>
          <div><strong>跨境出口</strong><span className="muted">未验证</span><small>不主动探测外部站点</small></div>
        </div>
        <p className="hint">本页所说 Global Network 指跨境代理基础设施，不指 ZeroTier、VLAN 或节点拓扑。网卡与路由在「网络状态」，平台链路在「平台」。</p>
      </article>
      <div className="filters"><label htmlFor="proxy-node">查看节点</label><select id="proxy-node" value={selectedNode} onChange={(event) => setSelectedNode(event.target.value)}><option value="">所有节点</option>{nodes.map((node) => <option key={node.id} value={node.id}>{node.hostname}</option>)}</select></div>
      {visibleNodes.map((node) => {
        const services = node.metrics?.proxy_services ?? [];
        const accounts = new Set(services.map((service) => service.uid));
        return <article className="data-panel" key={node.id}>
          <header className="panel-header"><div><h2>{node.hostname} <NodeStatus node={node} /></h2><p><SnapshotAge node={node} inventory /></p></div><span className={inventoryFresh(node) ? "positive" : "muted"}>{inventoryFresh(node) ? "当前观测" : "历史或未知"}</span></header>
          <p>{services.length} 个可见代理进程 · {accounts.size} 个用户上下文 · Agent {node.metrics?.privilege?.mode ?? "未知"} 权限</p>
          {!services.length && <p className="table-empty">当前权限下未观测到已识别的代理进程；不代表该节点没有代理。</p>}
          <CollectionNotes node={node} components={["proxy", "processes", "privilege"]} />
        </article>;
      })}
      <article className="data-panel">
        <header className="panel-header"><div><h2>代理实例与监听</h2><p>按节点、所属用户和进程列出；端口状态只比较当前筛选范围内的新鲜、可见监听。</p></div>
          <span className={`proxy-port-status proxy-port-status--${portStatus.tone}`} role="status"><i aria-hidden="true" />{portStatus.label}</span>
        </header>
        {portAnalysis.conflicts.length > 0 && <div className="proxy-port-conflicts" role="alert">
          <strong>重叠的代理监听：</strong>
          <ul>{portAnalysis.conflicts.slice(0, 8).map((conflict) => <li key={`${conflict.hostname}-${conflict.protocol}-${conflict.port}-${conflict.left.address}`}>
            {conflict.hostname} · {conflict.protocol.toUpperCase()} {conflict.port}：{conflict.left.username}/{conflict.left.name}（PID {conflict.left.pid}，{conflict.left.address}）与 {conflict.right.username}/{conflict.right.name}（PID {conflict.right.pid}，{conflict.right.address}）
          </li>)}</ul>
          {portAnalysis.conflicts.length > 8 && <small>另有 {portAnalysis.conflicts.length - 8} 组未展开。</small>}
        </div>}
        <div className="table-scroll"><table><thead><tr><th>节点</th><th>所属用户</th><th>代理进程</th><th>监听范围</th><th>可见监听地址</th><th>快照</th></tr></thead><tbody>
          {rows.map(({ node, service }) => <tr key={`${node.id}-${service.pid}-${service.name}`}>
            <td>{node.hostname}</td><td><strong>{service.username}</strong><small>UID {service.uid}</small></td>
            <td>{service.name}<small>PID {service.pid}</small></td>
            <td>{listenerScope(service.listen_addresses)}</td>
            <td>{service.listen_addresses.length ? service.listen_addresses.map((address) => <code className="block" key={address}>{address}</code>) : "未监听或不可读取"}</td>
            <td className={inventoryFresh(node) ? "positive" : "muted"}>{inventoryFresh(node) ? "新鲜" : "历史"}</td>
          </tr>)}
        </tbody></table></div>
        {!rows.length && <p className="table-empty">所选节点当前没有可见的代理实例。</p>}
        <p className="hint">“疑似冲突”指同一节点、同一协议与地址族中，不同代理进程的实际监听地址重叠；共享套接字等合法复用无法仅凭快照排除。不同节点、TCP 与 UDP 同号端口不算冲突，IPv4/IPv6 双栈重叠因 v6only 未知单独标为待确认。未发现可见冲突不代表配置端口未被其他进程占用。Basic Agent 可能看不到其他用户的进程或监听；不读取代理配置、环境变量、订阅和凭据，也不测试境外出口。</p>
      </article>
    </section>
  </>;
}
