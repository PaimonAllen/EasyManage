import { useState } from "react";
import { CollectionNotes, SnapshotAge } from "../components/NodeStatus";
import { PageHeading } from "../components/PageHeading";
import type { NodeSummary } from "../types/monitoring";

export function OSandSoftwares({ nodes }: { nodes: NodeSummary[] }) {
  const [query, setQuery] = useState("");
  return <>
    <PageHeading eyebrow="Operating systems & packages" title="OSandSoftwares" description="各节点实际系统版本、已安装软件包与运行中的 systemd 服务" />
    <section className="page-content">
      <label>搜索软件包 <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="名称或版本" /></label>
      {nodes.map((node) => {
        const packages = (node.metrics?.software_packages ?? []).filter((item) => `${item.name} ${item.version}`.toLowerCase().includes(query.toLowerCase()));
        return <article className="data-panel" key={node.id}>
          <header className="panel-header"><div><h2>{node.hostname}</h2><p><SnapshotAge node={node} inventory /></p></div></header>
          <div className="health-grid"><div><strong>系统</strong><span>{node.os_name} {node.os_version}</span><small>{node.architecture}</small></div>
            <div><strong>内核</strong><span>{node.kernel_version}</span><small>Agent {node.agent_version}</small></div>
            <div><strong>软件包 / 运行服务</strong><span>{node.metrics?.software_packages?.length ?? 0} / {node.metrics?.running_services?.length ?? 0}</span><small>来自节点当前清单</small></div></div>
          <h3 className="section-title">已安装软件包</h3>
          <div className="table-scroll"><table><thead><tr><th>名称</th><th>版本</th></tr></thead><tbody>{packages.slice(0, 150).map((item) => <tr key={item.name}><td>{item.name}</td><td><code>{item.version}</code></td></tr>)}</tbody></table></div>
          <p className="hint">显示匹配的 {Math.min(packages.length, 150)} / {packages.length} 项；按平台“扩展清单”频率采集（默认 60 秒）。尚无数据时请更新该节点 Agent。</p>
          <h3 className="section-title">运行中的服务</h3><div className="tags">{(node.metrics?.running_services ?? []).map((service) => <span key={service.name}>{service.name}</span>)}</div>
          <CollectionNotes node={node} components={["software"]} />
        </article>;
      })}
    </section>
  </>;
}
