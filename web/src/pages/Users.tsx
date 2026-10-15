import { useState } from "react";
import {
  CollectionNotes,
  NodeStatus,
  PrivilegeBadge,
  SnapshotAge,
} from "../components/NodeStatus";
import { PageHeading } from "../components/PageHeading";
import { UserManagement } from "../components/UserManagement";
import { accountState } from "../lib/accountStatus";
import type { OperationTarget } from "../api/client";
import type { NodeSummary, UserStatus } from "../types/monitoring";

const ADMIN_GROUPS = new Set(["sudo", "wheel", "lab-admins"]);
const SESSION_LABELS: Record<string, string> = {
  terminal: "终端",
  logind: "系统会话",
  ssh: "SSH 进程",
};

function loginDetails(user: UserStatus) {
  const evidence = user.session_evidence;
  if (!evidence) {
    return user.session_sources?.map((source) => SESSION_LABELS[source] ?? source).join(" · ") || "旧版 Agent 未提供来源计数";
  }
  const sources = [
    ["终端", evidence.terminal_records],
    ["系统会话", evidence.logind_sessions],
    ["SSH 进程", evidence.ssh_processes],
  ] as const;
  return sources.map(([label, count]) => `${label} ${count == null ? "未知" : count}`).join(" · ");
}

export function Users({ nodes, targets }: { nodes: NodeSummary[]; targets: OperationTarget[] }) {
  const [query, setQuery] = useState("");
  const [selectedNode, setSelectedNode] = useState("");
  const filteredNodes = nodes.filter(
    (node) => !selectedNode || node.id === selectedNode,
  );
  return (
    <>
      <PageHeading
        eyebrow="People & sessions"
        title="用户状态"
        description="root 与普通用户 · 排除系统账户 · 只读查看会话和账户基础状态"
      />
      <section className="page-content">
        <UserManagement targets={targets} />
        <div className="filters">
          <label className="sr-only" htmlFor="user-search">
            搜索用户名
          </label>
          <input
            id="user-search"
            placeholder="搜索用户名或用户组…"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
          <label className="sr-only" htmlFor="user-node">
            筛选节点
          </label>
          <select
            id="user-node"
            value={selectedNode}
            onChange={(event) => setSelectedNode(event.target.value)}
          >
            <option value="">所有节点</option>
            {nodes.map((node) => (
              <option key={node.id} value={node.id}>
                {node.hostname}
              </option>
            ))}
          </select>
        </div>
        <p className="hint">
          账户来源按系统普通用户 UID 范围识别，并始终包含 root；无法仅凭账户文件确认是否手工创建。登录线索分别列出终端记录、logind 会话和 SSH 进程，可覆盖部分 VS Code Remote-SSH 等非交互连接；来源可能重复，“至少”数量不可与来源计数相加，未检测到也不等于确定已退出。「状态」只核验登录 Shell 是否可执行，不代表密码、密钥或 SSH 策略一定可用。
        </p>
        {filteredNodes.map((node) => {
          const users = (node.metrics?.users ?? []).filter((user) =>
            `${user.username} ${user.groups.join(" ")}`
              .toLowerCase()
              .includes(query.toLowerCase()),
          );
          return (
            <article className="data-panel" key={node.id}>
              <header className="panel-header">
                <div>
                  <h2>
                    {node.hostname} <NodeStatus node={node} />
                  </h2>
                  <p>
                    <SnapshotAge node={node} inventory />
                  </p>
                </div>
                <PrivilegeBadge node={node} />
              </header>
              <div className="table-scroll">
                <table>
                  <thead>
                    <tr>
                      <th>用户 / UID</th>
                      <th>登录状态</th>
                      <th>进程</th>
                      <th>状态</th>
                      <th>用户组</th>
                      <th>Home / Shell</th>
                    </tr>
                  </thead>
                  <tbody>
                    {users.map((user) => (
                      <tr key={user.username}>
                        <td>
                          <strong>{user.username}</strong>
                          {user.uid === 0 && (
                            <span className="badge badge--full">root</span>
                          )}
                          {user.uid !== 0 && user.groups.some((group) => ADMIN_GROUPS.has(group)) && (
                            <span className="badge badge--admin">admin</span>
                          )}
                          <small>
                            UID {user.uid} · GID {user.gid}
                          </small>
                        </td>
                        <td>
                          <span
                            className={
                              (user.sessions ?? 0) > 0 ? "positive" : "muted"
                            }
                          >
                            {user.sessions == null
                              ? "登录状态未知"
                              : user.sessions > 0
                                ? `检测到登录 · 至少 ${user.sessions} 条`
                                : "未检测到登录"}
                          </span>
                          <small className="session-evidence">{loginDetails(user)}</small>
                        </td>
                        <td className="number">{user.process_count ?? "—"}</td>
                        <td title="仅核验登录 Shell；不代表密码、密钥或 SSH 策略可用">
                          <span className={accountState(user.account_status).className}>
                            {accountState(user.account_status).label}
                          </span>
                        </td>
                        <td>
                          <div className="tags">
                            {user.groups.map((group) => (
                              <span key={group}>{group}</span>
                            ))}
                          </div>
                        </td>
                        <td>
                          <code>{user.home}</code>
                          <small>{user.shell}</small>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                {users.length === 0 && (
                  <p className="table-empty">
                    {query
                      ? "没有匹配的用户"
                      : "暂未收到用户清单，请等待新版 Agent 采集。"}
                  </p>
                )}
              </div>
              <CollectionNotes
                node={node}
                components={["user", "session", "process"]}
              />
            </article>
          );
        })}
      </section>
    </>
  );
}
