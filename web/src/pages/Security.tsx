import { PageHeading } from "../components/PageHeading";
import { formatDate } from "../lib/format";
import type { NodeSummary } from "../types/monitoring";
import { securityFresh } from "../lib/collection";

function securityObservation(value: boolean | null | undefined, on: string, off: string) {
  return value == null ? "未知" : value ? on : off;
}

export function Security({ nodes }: { nodes: NodeSummary[] }) {
  return <>
    <PageHeading eyebrow="Managed host security" title="Security · 服务器安全" description="安全配置每小时采集一次；账户和 SSH 服务状态复用扩展清单，页面刷新不会触发重扫" />
    <section className="page-content">
      {nodes.map((node) => {
        const metrics = node.metrics;
        const security = metrics?.host_security;
        const inventoryAt = metrics?.inventory_collected_at;
        const hourly = security?.interval_seconds === 3600 && !!security.collected_at;
        const current = securityFresh(node);
        const securityAt = security?.collected_at ?? inventoryAt;
        const admins = (metrics?.users ?? []).filter((user) => user.groups.some((group) => ["sudo", "wheel", "lab-admins"].includes(group)));
        const sshRunning = (metrics?.running_services ?? []).some((service) => ["ssh.service", "sshd.service"].includes(service.name));
        const lockdown = security?.kernel_lockdown;
        return <article className="data-panel" key={node.id}>
          <header className="panel-header"><div><h2>{node.hostname}</h2><p>{node.os_name} {node.os_version} · Kernel {node.kernel_version} · 安全配置采集于 {formatDate(securityAt)}</p><p>{hourly ? "每小时一次 · 缓存结果" : "旧版采样 · 更新 Agent 后启用小时采集"}</p></div><span className={current ? "positive" : "muted"}>{current ? hourly ? "小时快照" : "旧版快照" : "历史或未知"}</span></header>
          <div className="health-grid">
            <div><strong>AppArmor</strong><span className={security?.apparmor_enabled ? "positive" : security?.apparmor_enabled === false ? "form-error" : "muted"}>{securityObservation(security?.apparmor_enabled, "已启用", "未启用")}</span><small>内核强制访问控制开关</small></div>
            <div><strong>ASLR</strong><span className={security?.aslr_mode === 2 ? "positive" : security?.aslr_mode == null ? "muted" : "form-error"}>{security?.aslr_mode == null ? "未知" : security.aslr_mode === 2 ? "完整随机化" : security.aslr_mode === 1 ? "部分随机化" : "已关闭"}</span><small>kernel.randomize_va_space = {security?.aslr_mode ?? "?"}</small></div>
            <div><strong>ptrace 限制</strong><span className={security?.ptrace_scope == null ? "muted" : security.ptrace_scope > 0 ? "positive" : "form-error"}>{security?.ptrace_scope == null ? "未知" : security.ptrace_scope > 0 ? "已限制" : "未限制"}</span><small>kernel.yama.ptrace_scope = {security?.ptrace_scope ?? "?"}</small></div>
            <div><strong>Kernel Lockdown</strong><span className={lockdown && lockdown !== "none" ? "positive" : lockdown == null ? "muted" : "form-error"}>{lockdown === "integrity" ? "Integrity" : lockdown === "confidentiality" ? "Confidentiality" : lockdown === "none" ? "未启用" : "未知"}</span><small>仅表示当前内核模式</small></div>
          </div>
          <div className="table-scroll"><table><thead><tr><th>观测项</th><th>结果</th><th>说明</th></tr></thead><tbody>
            <tr><td>Agent 权限</td><td>{metrics?.privilege?.mode ?? "未知"}</td><td>Basic 模式无法读取全部主机安全信息</td></tr>
            <tr><td>SSH 服务</td><td>{sshRunning ? "观测到运行" : "未观测到"}</td><td>服务运行不代表 SSH 配置安全；未观测到不等于已关闭</td></tr>
            <tr><td>管理组账户</td><td>{admins.length ? admins.map((user) => user.username).join(", ") : "未观测到"}</td><td>依据可见账户的 sudo / wheel / lab-admins 组信息</td></tr>
            <tr><td>防火墙规则</td><td>未验证</td><td>不以服务进程或端口可达性推断实际过滤规则</td></tr>
          </tbody></table></div>
          <p className="hint">账户和 SSH 服务清单采集于 {formatDate(inventoryAt)}；沿用扩展清单频率和各机器采集开关。</p>
          <p className="hint">这是当前 Agent 可读取的安全配置快照，不是漏洞扫描或合规结论。离线及过期数据仅供历史参考；未知项不可视作安全。</p>
        </article>;
      })}
      {!nodes.length && <p className="table-empty">尚无受管服务器安全数据。</p>}
    </section>
  </>;
}
