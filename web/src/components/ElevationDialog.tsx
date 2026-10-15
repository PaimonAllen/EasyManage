import { useEffect, useRef, useState } from "react";
import { elevateNode } from "../api/client";
import type { NodeSummary } from "../types/monitoring";

export function ElevationDialog({
  node,
  defaultUser,
  onClose,
  onSubmitted,
}: {
  node: NodeSummary;
  defaultUser?: string;
  onClose: () => void;
  onSubmitted: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [username, setUsername] = useState(defaultUser ?? "");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [showOriginDetails, setShowOriginDetails] = useState(false);
  const [message, setMessage] = useState("");
  const active = useRef(true);

  useEffect(() => {
    active.current = true;
    dialog.current?.showModal();
    return () => {
      active.current = false;
    };
  }, []);

  return (
    <dialog
      className="credentials-dialog"
      ref={dialog}
      onCancel={(event) => {
        if (busy) event.preventDefault();
        else onClose();
      }}
    >
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (busy) return;
          setBusy(true);
          setError("");
          setShowOriginDetails(false);
          const request = elevateNode(
            node.id,
            username.trim(),
            password,
            AbortSignal.timeout(90_000),
          );
          setPassword("");
          void request
            .then((result) => {
              if (!active.current) return;
              setMessage(
                result.message ||
                  "提权请求已执行，正在等待 Agent 上报新的权限状态。",
              );
              onSubmitted();
            })
            .catch((caught: unknown) => {
              if (active.current)
                setError(caught instanceof Error ? caught.message : "提权失败");
            })
            .finally(() => {
              if (active.current) setBusy(false);
            });
        }}
      >
        <div className="dialog-title">
          <div>
            <p className="eyebrow">Agent privileges</p>
            <h2>提升 {node.hostname}</h2>
          </div>
          <button
            type="button"
            className="icon-button"
            disabled={busy}
            aria-label="关闭"
            onClick={onClose}
          >
            ×
          </button>
        </div>
        <p className="hint">
          Full 会以 root 运行此节点的
          Agent，用于读取完整的系统指标。请输入目标机器上可通过 SSH
          登录且拥有管理员权限的账户。
        </p>
        {message ? (
          <div className="notice notice--success" role="status">
            {message}
            <p>最终状态以节点下一次心跳显示的 Full 为准。</p>
          </div>
        ) : (
          <>
            <label>
              管理员用户名
              <input
                required
                autoFocus
                name="username"
                autoComplete="off"
                maxLength={128}
                value={username}
                onChange={(event) => setUsername(event.target.value)}
                disabled={busy}
              />
            </label>
            <label>
              密码
              <input
                required
                type="password"
                maxLength={4096}
                name="password"
                autoComplete="off"
                value={password}
                onChange={(event) => setPassword(event.target.value)}
                disabled={busy}
              />
            </label>
            <p className="hint">
              密码仅用于本次操作，不保存在浏览器。LAN、Tailscale、ZeroTier
              地址在 HTTPS 证书覆盖后均可作为平台入口；操作请求必须与当前页面使用相同的地址和端口。
            </p>
          </>
        )}
        {error && (
          <>
            <p className="form-error" role="alert">
              {error}
              {error.startsWith("请从当前 HTTPS 平台页面发起操作") && (
                <button
                  type="button"
                  className="origin-error-info"
                  aria-label="查看同源校验详情"
                  aria-expanded={showOriginDetails}
                  aria-controls="elevation-origin-details"
                  title="查看同源校验详情"
                  onClick={() => setShowOriginDetails((value) => !value)}
                >
                  (!)
                </button>
              )}
            </p>
            {error.startsWith("请从当前 HTTPS 平台页面发起操作") && (
                <div
                  className="origin-error-details"
                  id="elevation-origin-details"
                  hidden={!showOriginDetails}
                >
                  <strong>同源校验详情</strong>
                  <p>
                    当前页面：<code>{window.location.origin}</code>
                  </p>
                  <p>
                    此操作使用相对路径调用平台 API，页面和 API 应使用相同的 HTTPS
                    主机与端口；物理网卡、Tailscale 和 ZeroTier 地址均按此规则处理。
                  </p>
                  <p>
                    收到这条服务端提示，表示请求的 <code>Origin</code> 与服务端收到的{" "}
                    <code>Host</code>（HTTP/1.1）或 <code>:authority</code>（HTTP/2）不一致，或 <code>Sec-Fetch-Site</code> 不是{" "}
                    <code>same-origin</code>。若通过反向代理访问，请检查代理是否保留原始
                    Host，并在浏览器开发者工具的请求标头中核对这些值。
                  </p>
                  <p>
                    如果是证书未覆盖当前 IP，浏览器通常会先报 HTTPS 连接错误，而不是返回此提示。
                  </p>
                </div>
              )}
          </>
        )}
        <div className="dialog-actions">
          <button
            type="button"
            className="button button--secondary"
            disabled={busy}
            onClick={onClose}
          >
            {message ? "关闭" : "取消"}
          </button>
          {!message && (
            <button
              type="submit"
              className="button"
              disabled={busy || !username.trim() || !password}
            >
              {busy ? "正在提权…" : "确认提升到 Full"}
            </button>
          )}
        </div>
      </form>
    </dialog>
  );
}
