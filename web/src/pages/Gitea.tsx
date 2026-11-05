import { useEffect, useState } from "react";
import { fetchGitea, fetchGiteaAccounts, type GiteaAccount, type GiteaAccounts, type GiteaStatus } from "../api/client";
import { PageHeading } from "../components/PageHeading";
import { formatDate } from "../lib/format";

export function Gitea() {
  const [data, setData] = useState<GiteaStatus | null>(null);
  const [accounts, setAccounts] = useState<GiteaAccounts | null>(null);
  const [accountsSupported, setAccountsSupported] = useState(true);
  const [error, setError] = useState("");
  const [accountError, setAccountError] = useState("");
  useEffect(() => {
    const controller = new AbortController();
    const refresh = () => void fetchGitea(controller.signal).then((result) => { setData(result); setError(""); })
      .catch((caught: unknown) => { if (!controller.signal.aborted) setError(caught instanceof Error ? caught.message : "Gitea 状态读取失败"); });
    refresh();
    const timer = window.setInterval(refresh, 20_000);
    return () => { controller.abort(); window.clearInterval(timer); };
  }, []);
  useEffect(() => {
    const controller = new AbortController();
    const refresh = () => void fetchGiteaAccounts(controller.signal)
      .then((result) => { setAccounts(result); setAccountsSupported(result !== null); setAccountError(""); })
      .catch((caught: unknown) => { if (!controller.signal.aborted) setAccountError(caught instanceof Error ? caught.message : "Gitea 账户读取失败"); });
    refresh();
    const timer = window.setInterval(refresh, 300_000);
    return () => { controller.abort(); window.clearInterval(timer); };
  }, []);
  const original = `http://${window.location.hostname}:3000/`;
  return <>
    <PageHeading eyebrow="Source hosting" title="Gitea 状态" description="查看本机 Gitea 健康检查、账户清单与账户状态" >
      <a href={original} target="_blank" rel="noopener noreferrer" className="button button--secondary">打开 Gitea ↗</a>
    </PageHeading>
    <section className="page-content"><article className="data-panel">
      {error && <div className="alert" role="alert">{error}</div>}
      <div className="health-grid"><div><strong>服务</strong><span className={data?.healthy ? "positive" : "form-error"}>{data?.message ?? "正在检查…"}</span><small>检查时间 {formatDate(data?.generated_at)}</small></div>
        <div><strong>版本</strong><span>{data?.version ?? "—"}</span><small>来自 /api/v1/version</small></div>
        <div><strong>响应时间</strong><span>{data?.latency_ms ?? "—"} ms</span><small>{data?.source ?? "本机健康接口"}</small></div></div>
      <GiteaDependencyChecks checks={data?.checks ?? null}/>
      <p className="hint">仅展示 Gitea 自身的健康检查。仓库或用户数据由 Gitea 的权限控制。</p>
    </article>
      {!accountsSupported && <article className="data-panel"><p className="hint">Gitea 账户清单将在平台后端更新后显示；现有健康检查不受影响。</p></article>}
      {accountsSupported && <article className="data-panel">
        <header className="panel-header"><div><h2>Gitea 账户状态</h2><p>只读账户清单 · 每 5 分钟刷新 · {accounts ? `采集于 ${formatDate(accounts.generated_at)}` : "正在读取…"}</p></div>
          <span className={accounts?.scope === "complete" ? "positive" : "muted"}>{accounts?.scope === "complete" ? "完整核验" : accounts?.scope === "partial" ? "部分核验" : accounts?.scope === "public" ? "公开可见" : "未核验"}</span></header>
        {accountError && <div className="alert" role="alert">{accountError}</div>}
        {accounts && <p className={accounts.scope === "complete" ? "hint" : "alert"}>{accounts.message}</p>}
        <GiteaAccountSummary accounts={accounts}/>
        <div className="table-scroll"><table><thead><tr><th>账户</th><th>角色</th><th>状态</th><th>建立时间</th><th>最后登录</th></tr></thead><tbody>
          {accounts?.users.map((user) => <tr key={user.id}>
            <td><strong>{user.login}</strong>{user.full_name && <small>{user.full_name}</small>}<small>ID {user.id}</small></td>
            <td>{user.is_admin === true ? "管理员" : user.is_admin === false ? "普通用户" : "待核验"}</td>
            <td className={user.account_status === "normal" ? "positive" : user.account_status === "unknown" ? "muted" : "form-error"}>{accountStatusLabel(user.account_status)}</td>
            <td>{formatDate(user.created)}</td><td>{accounts.scope === "complete" || accounts.scope === "partial" ? formatLastLogin(user.last_login) : "待核验"}</td>
          </tr>)}
        </tbody></table></div>
        {accounts?.users.length === 0 && <p className="table-empty">{accounts.scope === "unavailable" ? "账户数据暂不可用。" : "当前范围内没有可见账户。"}</p>}
        <p className="hint">“正常”仅表示 Gitea 的账户标志正常，不验证密码、SSH Key、仓库权限或实际登录。未配置管理员只读令牌时，只能显示公开可见账户，所有状态均为“待核验”。不展示邮箱及其他私有资料。</p>
      </article>}</section>
  </>;
}

export function GiteaDependencyChecks({checks}:{checks:GiteaStatus["checks"]}) {
  const rows=Object.entries(checks??{}).flatMap(([name,items])=>items.map((item,index)=>
    <tr key={`${name}-${index}`}><td>{name==="database:ping"?"数据库连接":name==="cache:ping"?"缓存连接":name}<small>{name}</small></td><td className={item.status==="pass"?"positive":"form-error"}>{item.status==="pass"?"正常":item.status==="warn"?"警告":"失败"}</td><td>{formatDate(item.time)}</td></tr>));
  return <><h3 className="section-title">依赖检查</h3><div className="table-scroll"><table><thead><tr><th>检查</th><th>状态</th><th>时间</th></tr></thead><tbody>
    {rows.length?rows:<tr><td colSpan={3} className="muted">暂未取得依赖检查结果</td></tr>}
  </tbody></table></div></>;
}

export function GiteaAccountSummary({accounts}:{accounts:GiteaAccounts|null}) {
  const users=accounts?.scope!=="unavailable"?accounts?.users:undefined;
  return <div className="health-grid">
    <div><strong>账户</strong><span>{users?.length??"—"}</span><small>{accounts?.scope==="unavailable"?"暂不可用":accounts?.scope==="complete"?"已完整核验":"当前可见，不一定是总数"}</small></div>
    <div><strong>正常</strong><span className={users?"positive":"muted"}>{users?.filter(user=>user.account_status==="normal").length??"—"}</span><small>已激活、可登录、未受限</small></div>
    <div><strong>需关注</strong><span>{users?.filter(user=>user.account_status!=="normal"&&user.account_status!=="unknown").length??"—"}</span><small>未激活、禁止登录或受限</small></div>
    <div><strong>待核验</strong><span>{users?.filter(user=>user.account_status==="unknown").length??"—"}</span><small>公开接口不用于判断状态</small></div>
  </div>;
}

function accountStatusLabel(status: GiteaAccount["account_status"]): string {
  switch (status) {
    case "normal": return "正常";
    case "inactive": return "未激活";
    case "prohibited": return "禁止登录";
    case "restricted": return "受限";
    default: return "待核验";
  }
}

function formatLastLogin(value: string | null): string {
  if (!value || /^0001-/.test(value)) return "无记录";
  return formatDate(value);
}
