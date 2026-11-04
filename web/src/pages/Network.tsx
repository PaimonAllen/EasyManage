import { NodePanel } from "../components/NodePanel";
import { useId, useState } from "react";
import {
  CollectionNotes,
  NodeStatus,
  PrivilegeBadge,
  SnapshotAge,
} from "../components/NodeStatus";
import { PageHeading } from "../components/PageHeading";
import { formatBytes, formatRate } from "../lib/format";
import type { NodeSummary, OverlayNetworkStatus } from "../types/monitoring";
import { localOverlayNetworks } from "../lib/overlayNetworks";

const overlayProviders = ["tailscale", "zerotier"] as const;

import { inventoryFresh } from "../lib/collection";

function interfaceKind(kind: string) {
  return ({ ethernet: "有线", wifi: "Wi-Fi", tailscale: "Tailscale", zerotier: "ZeroTier", vlan: "VLAN" } as Record<string, string>)[kind] ?? kind;
}

function overlayLabel(status?: OverlayNetworkStatus) {
  if (!status) return "未采集";
  return ({ healthy: "健康", not_joined: "暂未加入网络", degraded: "异常 / 降级", offline: "离线", unknown: "无法判定", not_installed: "未安装" } as Record<string, string>)[overlayState(status)] ?? "未采集";
}

function overlayState(status?: OverlayNetworkStatus): OverlayNetworkStatus["state"] {
  if (!status) return "unknown";
  // Older Agents report an unjoined ZeroTier network as degraded and a
  // Tailscale login requirement as offline. Keep the UI correct during rollout.
  if (status.provider === "zerotier" && status.state === "degraded" &&
      !status.networks.some((network) => network.state === "OK")) return "not_joined";
  if (status.provider === "tailscale" && status.state === "offline" &&
      status.detail.includes("需要登录")) return "not_joined";
  return status.state;
}

export function OverlayLocalDetails({ node, provider }: { node: NodeSummary; provider: OverlayNetworkStatus["provider"] }) {
  const status = node.metrics?.overlay_networks?.find((item) => item.provider === provider);
  const rows = localOverlayNetworks(provider, status, node.metrics?.interfaces ?? []);
  const fresh = inventoryFresh(node);
  return <div className="overlay-local-details">
    <p className={fresh ? "muted" : "stale-note"}>{fresh
      ? "本机网卡快照 · 入网状态来自 CLI，网卡 UP 不代表节点互通。"
      : "历史快照：节点离线或清单已过期，以下 IP 和状态不代表当前连接。"}</p>
    {rows.length === 0 && <p>暂未上报本机虚拟网卡或 IP；当前服务状态见上方，不以缺少网卡判定离线。</p>}
    {rows.map((row, index) => {
      const membershipState = row.membership?.state || (provider === "tailscale" ? overlayLabel(status) : "未上报");
      const stateClass = !fresh || (provider === "zerotier" && !row.membership) ? "unknown"
        : row.membership && row.membership.state !== "OK" ? "degraded" : overlayState(status);
      return <section className="overlay-local-details__network" key={`${row.membership?.network_id ?? "interface"}-${row.interface?.name ?? index}`}>
        <strong>{row.membership?.name || (provider === "tailscale" ? "Tailscale 虚拟网络" : row.membership?.network_id || "ZeroTier 虚拟网卡")}</strong>
        <dl>
          {row.membership && <><dt>网络 ID</dt><dd><code>{row.membership.network_id}</code></dd></>}
          <dt>入网状态</dt><dd className={`overlay-health-card__state--${stateClass}`}>{fresh ? membershipState : `历史 · ${membershipState}`}</dd>
          <dt>本机接口</dt><dd><code>{row.interface?.name || row.membership?.interface || "未上报"}</code></dd>
          <dt>本机 IP / 前缀</dt><dd className="overlay-local-details__addresses">{row.interface?.addresses.length
            ? [...new Set(row.interface.addresses)].map((address) => <code key={address}>{address}</code>)
            : <span className="muted">未上报 IP</span>}</dd>
          <dt>网卡状态</dt><dd>{row.interface?.state.toUpperCase() || "未知"}</dd>
          {row.interface?.vlan_id != null && <><dt>VLAN Tag</dt><dd>{row.interface.vlan_id}</dd></>}
          {row.interface?.mtu != null && <><dt>MTU</dt><dd>{row.interface.mtu}</dd></>}
          {row.interface?.master && <><dt>主接口</dt><dd><code>{row.interface.master}</code></dd></>}
        </dl>
      </section>;
    })}
  </div>;
}

export function OverlayHealthCard({ node, provider }: { node: NodeSummary; provider: OverlayNetworkStatus["provider"] }) {
  const [expanded, setExpanded] = useState(false);
  const detailsId = useId();
  const status = node.metrics?.overlay_networks?.find((item) => item.provider === provider);
  const fresh = inventoryFresh(node);
  const name = provider === "tailscale" ? "Tailscale" : "ZeroTier";
  return <div className="overlay-health-card">
    <div className="overlay-health-card__heading">
      <strong>{name}</strong>
      <div className="overlay-health-card__actions">
        <span className={`overlay-health-card__state overlay-health-card__state--${fresh ? overlayState(status) : "unknown"}`}>{fresh ? overlayLabel(status) : "历史 / 未知"}</span>
        <button type="button" className="button button--secondary button--small" aria-expanded={expanded} aria-controls={detailsId}
          aria-label={`${node.hostname} ${name} ${expanded ? "收起" : "展开"}本机 IP 和网络详情`} onClick={() => setExpanded((value) => !value)}>{expanded ? "收起" : "展开"}</button>
      </div>
    </div>
    <p>{status ? (fresh ? status.detail : `上次采集：${status.detail}`) : "等待新版 Agent 上报 CLI 状态"}</p>
    {status?.networks.length ? <ul className="overlay-health-card__networks">
      {status.networks.map((network) => <li key={network.network_id}>
        <span>{network.name || network.network_id}{network.interface ? ` · ${network.interface}` : ""}</span><code>{network.state}</code>
      </li>)}
    </ul> : null}
    <div id={detailsId} hidden={!expanded}>{expanded && <OverlayLocalDetails node={node} provider={provider} />}</div>
  </div>;
}

export function Network({ nodes }: { nodes: NodeSummary[] }) {
  const [selectedNode, setSelectedNode] = useState("");
  return (
    <>
      <PageHeading
        eyebrow="Connectivity & services"
        title="网络状态"
        description="有线 / Wi-Fi、Tailscale / ZeroTier 官方 CLI 健康、VLAN 与路由；跨境代理请见 Global Network"
      />
      <section className="page-content">
        <div className="filters">
          <label htmlFor="network-node">查看节点</label>
          <select
            id="network-node"
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
        {nodes
          .filter((node) => !selectedNode || node.id === selectedNode)
          .map((node) => {
            const overlays = node.metrics?.overlay_networks ?? [];
            const fresh = inventoryFresh(node);
            return (
            <NodePanel node={node} key={node.id} header={
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
          }>{() => <>
              <h3 className="section-title">组网服务健康</h3>
              <p className="section-hint">Tailscale 读取 <code>tailscale status --json</code>；ZeroTier 读取 <code>zerotier-cli -j info / listnetworks</code>。网卡 UP 不等于服务健康；这里也不主动测试节点互通。</p>
              <div className="overlay-health-grid">
                {overlayProviders.map((provider) => <OverlayHealthCard key={provider} node={node} provider={provider} />)}
              </div>
              <h3 className="section-title">网卡与虚拟网络</h3>
              <div className="table-scroll">
                <table>
                  <thead>
                    <tr>
                      <th>接口 / 类型</th>
                      <th>状态</th>
                      <th>地址 / MAC</th>
                      <th>VLAN / 主接口 / MTU</th>
                      <th>实时下行 / 上行</th>
                      <th>累计接收 / 发送</th>
                    </tr>
                  </thead>
                  <tbody>
                    {node.metrics?.interfaces?.map((item) => {
                      const metric = node.metrics?.networks.find(
                        (network) => network.interface === item.name,
                      );
                      const isOverlay = item.kind === "tailscale" || item.kind === "zerotier";
                      const overlay = isOverlay
                        ? overlays.find((status) => status.provider === item.kind)
                        : undefined;
                      return (
                        <tr key={item.name}>
                          <td>
                            <strong>{item.name}</strong>
                            <small>{item.kind ? interfaceKind(item.kind) : "未知类型"}</small>
                          </td>
                          <td>
                            {isOverlay ? (
                              <span
                                className={`overlay-health-card__state--${fresh ? overlayState(overlay) : "unknown"}`}
                                title={fresh ? overlay?.detail : "采集数据已过期"}
                              >
                                {fresh ? overlayLabel(overlay) : "历史 / 未知"}
                              </span>
                            ) : (
                              <span className={item.state.toLowerCase() === "up" ? "positive" : "muted"}>
                                {item.state || "未知"}
                              </span>
                            )}
                          </td>
                          <td>
                            {item.addresses.map((address) => (
                              <code className="block" key={address}>
                                {address}
                              </code>
                            ))}
                            {!item.addresses.length && (
                              <span className="muted">无已上报 IP</span>
                            )}
                            <small>{item.mac_address ?? "—"}</small>
                          </td>
                          <td>
                            {item.vlan_id == null
                              ? "—"
                              : `VLAN ${item.vlan_id}`}
                            <small>
                              {item.master ?? "—"} · MTU {item.mtu ?? "—"}
                            </small>
                          </td>
                          <td>
                            <span>
                              ↓{" "}
                              {metric
                                ? formatRate(metric.received_bytes_per_second)
                                : "—"}
                            </span>
                            <small>
                              ↑{" "}
                              {metric
                                ? formatRate(
                                    metric.transmitted_bytes_per_second,
                                  )
                                : "—"}
                            </small>
                          </td>
                          <td>
                            {metric
                              ? formatBytes(metric.total_received_bytes)
                              : "—"}
                            <small>
                              {metric
                                ? formatBytes(metric.total_transmitted_bytes)
                                : "—"}
                            </small>
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
                {!node.metrics?.interfaces?.length && (
                  <p className="table-empty">暂未收到接口清单。</p>
                )}
              </div>
              <details className="network-route-details" aria-label={`${node.hostname} 路由`}>
              <summary className="section-title">路由 · {node.metrics?.routes?.length ?? 0} 条</summary>
              <div className="table-scroll">
                <table>
                  <thead>
                    <tr>
                      <th>目标</th>
                      <th>网关</th>
                      <th>接口</th>
                      <th>Metric</th>
                    </tr>
                  </thead>
                  <tbody>
                    {node.metrics?.routes?.map((route, index) => (
                      <tr key={`${route.destination}-${index}`}>
                        <td>
                          <code>{route.destination}</code>
                        </td>
                        <td>
                          <code>{route.gateway ?? "直连"}</code>
                        </td>
                        <td>{route.interface ?? "—"}</td>
                        <td className="number">{route.metric ?? "—"}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                {!node.metrics?.routes?.length && (
                  <p className="table-empty">未采集到路由。</p>
                )}
              </div>
              </details>
              <CollectionNotes
                node={node}
                components={["network", "interface", "route"]}
              />
            </>}</NodePanel>
          );
          })}
      </section>
    </>
  );
}
