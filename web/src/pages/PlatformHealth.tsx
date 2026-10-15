import { useEffect, useState } from "react";
import { fetchPlatformHealth, fetchPlatformPerformance, type PlatformHealthStatus, type PlatformPerformanceStatus, type ProcessMetricSummary, type ProcessWindowSummary, type StorageWindowSummary } from "../api/client";
import { PageHeading } from "../components/PageHeading";
import { formatBytes, formatDate, formatRate } from "../lib/format";
import type { PlatformProcessMetric } from "../types/monitoring";
import { CollectionSettings } from "../components/CollectionSettings";

function ProcessCells({ usage }: { usage?: PlatformProcessMetric | null }) {
  return <>
    <td className="number">{usage?.cpu_percent_of_host == null ? "—" : `${usage.cpu_percent_of_host.toFixed(2)}%`}</td>
    <td className="number">{usage?.resident_memory_bytes == null ? "—" : formatBytes(usage.resident_memory_bytes)}</td>
    <td className="number">{usage?.disk_read_bytes_per_second == null ? "—" : formatRate(usage.disk_read_bytes_per_second)}</td>
    <td className="number">{usage?.disk_written_bytes_per_second == null ? "—" : formatRate(usage.disk_written_bytes_per_second)}</td>
  </>;
}

function SummaryCell({ metric, format }: { metric: ProcessMetricSummary; format: (value: number) => string }) {
  const zeroSamples = metric.samples > 0 && metric.average === 0 && metric.maximum === 0;
  return <td className="number" title={metric.samples > 0 && metric.average != null && metric.maximum != null ? `原始均值 ${metric.average}；原始最大值 ${metric.maximum}${zeroSamples ? "；有效采样均为 0，并非缺失补零" : ""}` : "尚无有效采样"}>{metric.samples > 0 && metric.average != null && metric.maximum != null
    ? <><strong>均值 {format(metric.average)}</strong><small className="performance-maximum">最大 {format(metric.maximum)}</small><small className="performance-samples">{metric.samples.toLocaleString("zh-CN")} 样本{zeroSamples && " · 采样均为 0"}</small></>
    : <span className="muted">—</span>}</td>;
}

function formatCpu(value: number): string {
  if (value > 0 && value < 0.001) return `${Number(value.toPrecision(3))}%`;
  return `${value.toFixed(value < 1 ? 3 : 2)}%`;
}

export function StorageCell({ storage, server }: { storage?: StorageWindowSummary | null; server: boolean }) {
  const total = storage?.total_bytes;
  const latest = storage?.latest;
  return <td className="number platform-storage-cell">
    {total && total.samples > 0 && total.average != null && total.maximum != null
      ? <><strong>均值 {formatBytes(total.average)}</strong><small className="performance-maximum">最大 {formatBytes(total.maximum)}</small><small className="performance-samples">{total.samples.toLocaleString("zh-CN")} 样本</small></>
      : <span className="muted">—</span>}
    {latest && <><small className="performance-sample-time">最近 {formatDate(latest.collected_at)}</small>
      <small className="performance-samples">{server
        ? `数据库 ${latest.database_bytes == null ? "—" : formatBytes(latest.database_bytes)} · 网页 ${latest.web_bytes == null ? "—" : formatBytes(latest.web_bytes)} · 程序 ${latest.binary_bytes == null ? "—" : formatBytes(latest.binary_bytes)}`
        : `SQLite ${latest.state_bytes == null ? "—" : formatBytes(latest.state_bytes)} · 程序 ${latest.binary_bytes == null ? "—" : formatBytes(latest.binary_bytes)}`}</small></>}
  </td>;
}

function PerformanceRow({ hostname, component, summary, current, storage, server }: {
  hostname: string;
  component: string;
  summary: ProcessWindowSummary;
  current?: PlatformProcessMetric | null;
  storage?: StorageWindowSummary | null;
  server: boolean;
}) {
  return <tr>
    <td><strong>{hostname}</strong><small>{component}{current ? ` · 当前 PID ${current.pid}` : ""}</small></td>
    <td>{summary.sample_count ? <><strong>{summary.sample_count.toLocaleString("zh-CN")} 个样本</strong><small className="performance-sample-time">起 {formatDate(summary.first_sample_at)}</small><small className="performance-sample-time">止 {formatDate(summary.last_sample_at)}</small></> : <span className="muted">尚无历史样本</span>}</td>
    <SummaryCell metric={summary.cpu_percent_of_host} format={formatCpu} />
    <SummaryCell metric={summary.resident_memory_bytes} format={formatBytes} />
    <SummaryCell metric={summary.disk_read_bytes_per_second} format={formatRate} />
    <SummaryCell metric={summary.disk_written_bytes_per_second} format={formatRate} />
    <StorageCell storage={storage} server={server} />
  </tr>;
}

export function PlatformHealth() {
  const [data, setData] = useState<PlatformHealthStatus | null>(null);
  const [performance, setPerformance] = useState<PlatformPerformanceStatus | null>(null);
  const [error, setError] = useState("");
  const [performanceError, setPerformanceError] = useState("");
  const [performanceSupported, setPerformanceSupported] = useState(true);
  useEffect(() => {
    const controller = new AbortController();
    const refresh = () => void fetchPlatformHealth(controller.signal)
      .then((result) => { setData(result); setError(""); })
      .catch((caught: unknown) => { if (!controller.signal.aborted) setError(caught instanceof Error ? caught.message : "平台健康检查失败"); });
    refresh();
    const timer = window.setInterval(refresh, 15_000);
    return () => { controller.abort(); window.clearInterval(timer); };
  }, []);
  useEffect(() => {
    const controller = new AbortController();
    const refresh = () => void fetchPlatformPerformance(controller.signal)
      .then((result) => { setPerformance(result); setPerformanceSupported(result !== null); setPerformanceError(""); })
      .catch((caught: unknown) => { if (!controller.signal.aborted) setPerformanceError(caught instanceof Error ? caught.message : "平台进程历史读取失败"); });
    refresh();
    const timer = window.setInterval(refresh, 60_000);
    return () => { controller.abort(); window.clearInterval(timer); };
  }, []);
  return <>
    <PageHeading eyebrow="Control plane" title="平台" description="平台状态、节点链路、自身性能与采集设置；主机安全状态请见 Security">
      <button type="button" className="button button--secondary button--small" onClick={() => document.getElementById("platform-collection-settings")?.scrollIntoView({ block: "start" })}>采集设置</button>
    </PageHeading>
    <section className="page-content">
      {error && <div className="alert" role="alert">{error}</div>}
      <article className="data-panel">
        <header className="panel-header"><div><h2>平台自身</h2><p>最近检查 {formatDate(data?.generated_at)}</p></div></header>
        <div className="health-grid">
          <div><strong>HTTPS API</strong><span className={data?.platform.api_healthy ? "positive" : "muted"}>{data?.platform.api_healthy ? "正常" : "未知"}</span><small>版本 {data?.platform.version ?? "—"}</small></div>
          <div><strong>PostgreSQL</strong><span className={data?.platform.database_healthy ? "positive" : "form-error"}>{data?.platform.database_healthy ? "正常" : "不可用"}</span><small>检查耗时 {data?.platform.database_latency_ms ?? "—"} ms</small></div>
          <div><strong>节点注册表</strong><span className={data?.target_error ? "form-error" : "positive"}>{data?.target_error ?? "正常"}</span><small>{data?.platform_to_nodes.length ?? 0} 个可信 SSH 目标</small></div>
        </div>
      </article>
      <article className="data-panel">
        <header className="panel-header"><div><h2>节点链路状态</h2><p>一行对应一个节点，分别展示 Agent 回传心跳与平台发起的 SSH 端口检查。</p></div></header>
        <div className="table-scroll"><table><thead><tr><th>节点</th><th>Agent → 平台</th><th>平台 → 节点</th><th>版本</th><th>最近心跳</th></tr></thead><tbody>
          {data?.nodes.map((node) => {
            const link = data.platform_to_nodes.find((item) => item.node_id === node.node_id);
            return <tr key={node.node_id}><td><strong>{node.hostname}</strong><small>{link ? `${link.host}:${link.port}` : "未配置 SSH 目标"}</small></td>
              <td className={node.agent_to_platform_healthy ? "positive" : "form-error"}>{node.agent_to_platform_healthy ? "心跳正常" : "心跳过期"}</td>
              <td className={link?.ssh_tcp_reachable ? "positive" : "muted"}>{link ? (link.ssh_tcp_reachable ? "SSH 端口可达" : "SSH 端口不可达") : "未配置"}</td>
              <td>{node.agent_version}</td><td>{formatDate(node.last_seen)}</td></tr>;
          })}
        </tbody></table>{!data && <p className="table-empty">正在获取节点链路…</p>}</div>
        <p className="hint">SSH 端口可达只证明 TCP 连通；心跳证明 Agent 能向平台上报。这两项不代表已测试节点之间的连接，也不代表 SSH 身份认证成功。</p>
      </article>
      <article className="data-panel">
        <header className="panel-header"><div><h2>平台性能分析 · 过去 24 小时</h2><p>{performance ? `${formatDate(performance.window_start)} → ${formatDate(performance.window_end)} · 每 60 秒更新统计` : "Server 全进程与所有已注册 Agent 的历史采样均值及最大值"}</p></div></header>
        {performanceError && <div className="alert" role="alert">{performanceError}</div>}
        {!performanceSupported && <p className="table-empty">24 小时进程历史功能待平台后端更新；下方仍可查看最新采样。</p>}
        {performanceSupported && !performance && !performanceError && <p className="table-empty">正在读取过去 24 小时的进程样本…</p>}
        {performance && <div className="table-scroll"><table className="platform-performance-table"><thead><tr><th>机器 / 组件</th><th>观测样本范围</th><th>CPU / 整机</th><th>常驻内存</th><th>磁盘读取</th><th>磁盘写入</th><th>存储占用</th></tr></thead><tbody>
          <PerformanceRow hostname={performance.server.hostname} component="EasyManage Server（含本页监控开销）" summary={performance.server.stats} current={data?.platform_process} storage={performance.server.storage} server />
          {performance.agents.map((agent) => <PerformanceRow key={agent.node_id} hostname={agent.hostname} component={`Agent · ${agent.online ? "在线" : "离线"}`} summary={agent.stats} current={data?.nodes.find((node) => node.node_id === agent.node_id)?.agent_process} storage={agent.storage} server={false} />)}
        </tbody></table></div>}
        <p className="hint platform-performance-hint">每项显示过去 24 小时实际样本的均值、最大值和有效样本数；缺失时显示“—”，不补零，也不把上线前的数据伪装成 24 小时历史。存储占用每 1 小时只读采样一次：Server 统计程序文件、当前网页目录及 EasyManage 专用 PostgreSQL 数据库；Agent 统计程序文件及其 SQLite 主文件、WAL、SHM。显示文件逻辑大小，不扫描整盘或业务目录；共享 PostgreSQL/Nginx 的进程开销和共享数据库集群文件不归因于本平台。CPU 为进程占整机总 CPU 的比例，内存为 RSS，磁盘为该进程的 I/O 速率。</p>
        <details className="platform-current-samples"><summary>查看最新一次进程采样（非 24 小时统计）</summary>
          <div className="table-scroll"><table className="platform-process-table"><thead><tr><th>机器 / 组件</th><th>CPU / 整机</th><th>常驻内存</th><th>磁盘读取</th><th>磁盘写入</th><th>采集时间</th></tr></thead><tbody>
            <tr><td><strong>{data?.platform_hostname ?? "平台主机"}</strong><small>Server{data?.platform_process ? ` · PID ${data.platform_process.pid}` : ""}</small></td>
              <ProcessCells usage={data?.platform_process} /><td>{formatDate(data?.platform_process_collected_at ?? data?.generated_at)}</td></tr>
            {data?.nodes.map((node) => <tr key={node.node_id}><td><strong>{node.hostname}</strong><small>Agent{node.agent_process ? ` · PID ${node.agent_process.pid}` : ""}</small></td>
              <ProcessCells usage={node.agent_process} /><td>{formatDate(node.agent_process_collected_at)}</td></tr>)}
          </tbody></table></div>
        </details>
      </article>
      <CollectionSettings />
    </section>
  </>;
}
