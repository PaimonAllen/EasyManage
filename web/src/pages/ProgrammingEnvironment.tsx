import { NodePanel } from "../components/NodePanel";
import { useState } from "react";
import { CollectionNotes, NodeStatus, SnapshotAge } from "../components/NodeStatus";
import { PageHeading } from "../components/PageHeading";
import type { NodeSummary } from "../types/monitoring";
import { PROGRAMMING_CATEGORIES as CATEGORY_ORDER, PROGRAMMING_CATEGORY_HINTS, displayProgrammingCategory as displayCategory } from "../lib/programming";

import { inventoryFresh as fresh } from "../lib/collection";

export function ProgrammingEnvironment({ nodes }: { nodes: NodeSummary[] }) {
  const [category, setCategory] = useState("全部");
  const [showMissing, setShowMissing] = useState(false);
  const observedCategories = new Set(nodes.flatMap((node) => node.metrics?.programming_tools ?? []).map(displayCategory));
  const categories = ["全部", ...CATEGORY_ORDER, ...[...observedCategories].filter((value) => !CATEGORY_ORDER.includes(value)).sort()];
  const discovered = nodes.filter(fresh).flatMap((node) => node.metrics?.programming_tools ?? [])
    .filter((tool) => tool.executable_paths.length).length;
  return <>
    <PageHeading eyebrow="Read-only developer inventory" title="编程环境" description="按节点查看编译器、运行时和开发工具的可见安装情况" />
    <section className="page-content">
      <article className="data-panel">
        <header className="panel-header"><div><h2>当前观测</h2><p>工具按节点分别统计，清单按平台“扩展清单”频率更新（默认 60 秒）；不执行工具或读取配置。</p></div></header>
        <div className="health-grid">
          <div><strong>受管节点</strong><span>{nodes.length}</span><small>包括离线节点</small></div>
          <div><strong>新鲜清单</strong><span>{nodes.filter(fresh).length}</span><small>在线且在已应用的采集有效期内</small></div>
          <div><strong>发现的工具</strong><span>{discovered}</span><small>不同节点分别计数</small></div>
        </div>
        <p className="hint">检测范围：固定系统目录、Agent 运行用户的常见 bin 目录，以及 CUDA、ROCm、MATLAB 的常见安装目录。仅检查可执行文件元数据；版本仅显示已采集到的系统包版本，不能代表自定义安装的工具版本。“未发现”不等于未安装或不可用。</p>
      </article>
      <div className="programming-toolbar">
        <span className="programming-filter-label">语言 / 类别</span>
        <div className="programming-category-tags" role="group" aria-label="按编程语言或工具类别筛选">
          {categories.map((value) => <button key={value} type="button" className="programming-category-tag" aria-pressed={category === value} onClick={() => setCategory(value)}>{value}</button>)}
        </div>
        <label className="programming-range" htmlFor="programming-missing">显示范围
          <select id="programming-missing" value={showMissing ? "all" : "found"} onChange={(event) => setShowMissing(event.target.value === "all")}>
            <option value="found">仅已发现</option><option value="all">全部检测项</option>
          </select>
        </label>
      </div>
      {PROGRAMMING_CATEGORY_HINTS[category] && <p className="hint">{PROGRAMMING_CATEGORY_HINTS[category]}</p>}
      {nodes.map((node) => {
        const tools = node.metrics?.programming_tools;
        const categoryNotReported = !!tools?.length && category in PROGRAMMING_CATEGORY_HINTS && !tools.some((tool) => displayCategory(tool) === category);
        const visible = (tools ?? []).filter((tool) => (category === "全部" || displayCategory(tool) === category) && (showMissing || tool.executable_paths.length));
        return <NodePanel node={node} key={node.id} header={
          <header className="panel-header"><div><h2>{node.hostname} <NodeStatus node={node} /></h2><p><SnapshotAge node={node} inventory /></p></div><span className={fresh(node) ? "positive" : "muted"}>{fresh(node) ? "当前观测" : "历史或未知"}</span></header>
          }>{() => <>
          {tools?.length ? <>
            <div className="table-scroll"><table><thead><tr><th>工具</th><th>类别</th><th>状态</th><th>系统包版本</th><th>发现路径</th></tr></thead><tbody>
              {visible.map((tool) => <tr key={tool.name}>
                <td><strong>{tool.name}</strong></td><td><span className="programming-category-badge">{displayCategory(tool)}</span></td>
                <td className={fresh(node) && tool.executable_paths.length ? "positive" : "muted"}>{tool.executable_paths.length ? "发现可执行文件" : "限定路径未发现"}{!fresh(node) && " · 历史"}</td>
                <td>{tool.package_version ?? "未知"}</td>
                <td>{tool.executable_paths.length ? tool.executable_paths.map((path) => <code className="block" key={path}>{path}</code>) : "—"}</td>
              </tr>)}
            </tbody></table></div>
            {!visible.length && <p className="table-empty">{categoryNotReported ? `此节点 Agent 尚未上报“${category}”检测项，请在 Agent 管理中更新后等待清单采集。` : <>{category === "全部" ? "当前范围暂无发现的工具" : `“${category}”类别当前暂无发现的工具`}；可切换为“全部检测项”。</>}</p>}
          </> : <p className="table-empty">尚未收到新版 Agent 的编程环境清单。</p>}
          <CollectionNotes node={node} components={["software", "privilege"]} />
        </>}</NodePanel>;
      })}
      {!nodes.length && <p className="table-empty">尚无受管节点。</p>}
    </section>
  </>;
}
