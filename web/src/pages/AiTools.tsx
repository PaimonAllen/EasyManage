import { useState } from "react";
import { CollectionNotes, NodeStatus, SnapshotAge } from "../components/NodeStatus";
import { PageHeading } from "../components/PageHeading";
import type { AiToolStatus, NodeSummary } from "../types/monitoring";

import { inventoryFresh as isFresh } from "../lib/collection";
import { agentToolLabel, isAgentTool } from "../lib/aiTools";

function detected(tool: AiToolStatus) {
  return tool.executable_paths.length > 0 || tool.processes.length > 0 || tool.service_units.length > 0;
}

function observedState(tool: AiToolStatus) {
  if (tool.processes.length || tool.service_units.length) return "观测到运行";
  if (tool.executable_paths.length) return "已发现可执行文件";
  return "未在限定范围发现";
}

export function AiTools({ nodes }: { nodes: NodeSummary[] }) {
  const [showAll, setShowAll] = useState(false);
  const freshTools = nodes.filter(isFresh).flatMap((node) => node.metrics?.ai_tools ?? []).filter(isAgentTool);
  const installed = freshTools.filter((tool) => tool.executable_paths.length > 0).length;
  const processes = freshTools.reduce((total, tool) => total + tool.processes.length, 0);
  const services = freshTools.reduce((total, tool) => total + tool.service_units.length, 0);
  return <>
    <PageHeading eyebrow="Read-only Agent tool inventory" title="AI Agent 工具状态" description="按服务器查看 Claude Code、Codex 等 Agent 工具的安装与运行状态，不执行工具或读取凭据" />
    <section className="page-content">
      <article className="data-panel">
        <header className="panel-header"><div><h2>当前观测概览</h2><p>仅统计在线且清单新鲜的节点；同名工具在不同节点分别计数。</p></div></header>
        <div className="health-grid">
          <div><strong>已发现安装的工具</strong><span>{installed}</span><small>账户清单中的用户目录和固定系统路径</small></div>
          <div><strong>运行进程</strong><span>{processes}</span><small>仅识别白名单进程名</small></div>
          <div><strong>systemd 服务</strong><span>{services}</span><small>当前上报的运行单元</small></div>
        </div>
        <p className="hint">本页检测 Codex、Claude Code、Gemini CLI、OpenCode、Aider 这类 Agent 工具，不统计模型推理运行时、模型/API 提供方、WebUI 或编程语言环境。未发现不等于未安装；通用 Python / Node 进程不会被猜测为 Agent 工具。</p>
        <p className="hint">新版 Agent 检查账户清单内 /home/账户 与 /root 的常见安装位置：本地 bin、npm、Bun、OpenCode、pnpm、NVM / FNM / mise、Conda、pipx / uv。不递归扫描项目；每次最多 64 个账户目录、1024 个探测目录、每个版本目录最多 16 个环境，工作预算 2 秒。旧 Agent 保留原检测范围，但本页只展示 Agent 工具。</p>
      </article>
      <div className="filters"><label htmlFor="ai-show-all">显示范围</label><select id="ai-show-all" value={showAll ? "all" : "detected"} onChange={(event) => setShowAll(event.target.value === "all")}><option value="detected">仅已发现或运行</option><option value="all">全部已知工具</option></select></div>
      {nodes.map((node) => {
        const tools = node.metrics?.ai_tools?.filter(isAgentTool);
        const visible = (tools ?? []).filter((tool) => showAll || detected(tool));
        const incomplete = node.metrics?.collection_issues?.some((issue) => issue.component === "ai_tools" || issue.component === "inventory");
        return <article className="data-panel" key={node.id}>
          <header className="panel-header"><div><h2>{node.hostname} <NodeStatus node={node} /></h2><p><SnapshotAge node={node} inventory /></p>{incomplete && <p className="ai-scan-warning">扫描不完整 · 展开下方采集限制查看原因{!isFresh(node) && " · 历史"}</p>}</div><span className={isFresh(node) ? "positive" : "muted"}>{isFresh(node) ? "当前观测" : "历史或未知"}</span></header>
          {node.metrics?.ai_tools?.length ? <>
            <div className="table-scroll"><table><thead><tr><th>工具</th><th>类型</th><th>状态</th><th>可执行文件</th><th>运行进程 / 用户</th><th>运行服务</th></tr></thead><tbody>
              {visible.map((tool) => <tr key={tool.name}>
                <td><strong>{agentToolLabel(tool.name)}</strong><code className="block">{tool.name}</code></td><td>Agent 工具</td>
                <td className={isFresh(node) && (tool.processes.length || tool.service_units.length) ? "positive" : "muted"}>{observedState(tool)}{!isFresh(node) && " · 历史"}</td>
                <td>{tool.executable_paths.length ? tool.executable_paths.map((path) => <code className="block" key={path}>{path}</code>) : "未在限定路径发现"}</td>
                <td>{tool.processes.length ? tool.processes.map((process) => <span className="block" key={process.pid}>{process.username}（UID {process.uid}，PID {process.pid}）</span>) : "未观测到"}</td>
                <td>{tool.service_units.length ? tool.service_units.join(", ") : "未观测到"}</td>
              </tr>)}
            </tbody></table></div>
            {!visible.length && <p className="table-empty">当前权限和固定检测范围内，未发现可识别的 Agent 工具。</p>}
          </> : <p className="table-empty">尚未收到新版 Agent 的 AI 工具清单。</p>}
          <CollectionNotes node={node} components={["ai_tools", "processes", "privilege", "inventory"]} />
        </article>;
      })}
      {!nodes.length && <p className="table-empty">尚无受管节点。</p>}
      <p className="hint">“观测到运行”不代表模型/API 可用、账号已登录或请求成功。页面不读取命令行参数、环境变量、模型配置、会话、API Key，也不调用外部 AI 服务。Basic Agent 对其他用户安装目录和部分进程可能不可见。</p>
    </section>
  </>;
}
