import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { PageHeading } from "../components/PageHeading";
import type { OperationTarget } from "../api/client";
import type { NodeSummary } from "../types/monitoring";

type ConnectionStatus =
  "idle" | "connecting" | "connected" | "closed" | "error";
const statusLabels: Record<ConnectionStatus, string> = {
  idle: "未连接",
  connecting: "正在验证…",
  connected: "SSH 已连接",
  closed: "会话已结束",
  error: "连接失败",
};

export default function Operations({
  nodes,
  targets,
  enabled,
  targetsError,
}: {
  nodes: NodeSummary[];
  targets: OperationTarget[];
  enabled: boolean;
  targetsError: string;
}) {
  const [nodeId, setNodeId] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [status, setStatus] = useState<ConnectionStatus>("idle");
  const [error, setError] = useState("");
  const container = useRef<HTMLDivElement>(null);
  const session = useRef<{ dispose: () => void; focus: () => void } | null>(
    null,
  );
  const busy = status === "connecting" || status === "connected";
  const target = targets.find((item) => item.node_id === nodeId);

  useEffect(() => {
    if (!nodeId && targets.length) {
      setNodeId(targets[0].node_id);
      setUsername(targets[0].default_user);
    }
  }, [nodeId, targets]);

  useEffect(
    () => () => {
      session.current?.dispose();
      session.current = null;
    },
    [],
  );

  function connect() {
    if (!container.current || !nodeId || !username.trim() || !password || busy)
      return;
    session.current?.dispose();
    session.current = null;
    setError("");
    setStatus("connecting");
    const terminal = new Terminal({
      cursorBlink: true,
      fontSize: 14,
      fontFamily: '"Cascadia Code", "DejaVu Sans Mono", monospace',
      scrollback: 3_000,
      theme: {
        background: "#0b1016",
        foreground: "#d9e2ed",
        cursor: "#4de2c5",
        selectionBackground: "#264d50",
      },
      allowProposedApi: false,
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(container.current);
    fit.fit();
    terminal.writeln("\x1b[90m正在连接目标主机…\x1b[0m");
    const url = new URL(
      `/api/v1/nodes/${encodeURIComponent(nodeId)}/terminal`,
      window.location.href,
    );
    url.protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    let secret = password;
    setPassword("");
    let socket: WebSocket;
    try {
      socket = new WebSocket(url);
    } catch {
      secret = "";
      terminal.dispose();
      setError("浏览器无法创建终端连接，请检查页面连接协议。");
      setStatus("error");
      return;
    }
    let active = true;
    let ready = false;
    let failed = false;
    const timer = window.setTimeout(() => {
      if (active && !ready) {
        secret = "";
        failed = true;
        setError("SSH 认证超时，请检查目标连接及账户信息。");
        setStatus("error");
        socket.close();
      }
    }, 30_000);

    const input = terminal.onData((data) => {
      if (new TextEncoder().encode(data).length > 8_192) {
        setError("单次终端输入超过 8 KiB，未发送。请分段粘贴。");
        return;
      }
      if (ready && socket.readyState === WebSocket.OPEN)
        socket.send(JSON.stringify({ type: "input", data }));
    });
    const resize = terminal.onResize(({ cols, rows }) => {
      if (ready && socket.readyState === WebSocket.OPEN)
        socket.send(JSON.stringify({ type: "resize", cols, rows }));
    });
    const observer = new ResizeObserver(() => {
      if (active) fit.fit();
    });
    observer.observe(container.current);

    socket.onopen = () => {
      if (!active || failed) return socket.close();
      socket.send(
        JSON.stringify({
          type: "auth",
          username: username.trim(),
          password: secret,
          cols: terminal.cols,
          rows: terminal.rows,
        }),
      );
      secret = "";
    };
    socket.onmessage = (event: MessageEvent) => {
      if (!active || typeof event.data !== "string") return;
      let message: { type: string; data?: string; message?: string };
      try {
        message = JSON.parse(event.data) as typeof message;
        if (!message || typeof message.type !== "string")
          throw new Error("Invalid terminal frame");
      } catch {
        failed = true;
        setError("终端响应格式异常，连接已关闭。");
        setStatus("error");
        socket.close();
        return;
      }
      if (message.type === "ready") {
        ready = true;
        window.clearTimeout(timer);
        setStatus("connected");
        terminal.clear();
        terminal.focus();
      } else if (
        message.type === "output" &&
        typeof message.data === "string"
      ) {
        terminal.write(message.data);
      } else if (message.type === "error") {
        ready = false;
        failed = true;
        window.clearTimeout(timer);
        setError(message.message ?? "SSH 连接失败。");
        setStatus("error");
        socket.close();
      } else if (message.type === "closed") {
        ready = false;
        setStatus("closed");
        socket.close();
      }
    };
    socket.onerror = () => {
      if (!active) return;
      secret = "";
      ready = false;
      failed = true;
      window.clearTimeout(timer);
      setError("无法建立终端连接，请检查平台 HTTPS 和目标 SSH 服务。");
      setStatus("error");
    };
    socket.onclose = () => {
      secret = "";
      ready = false;
      window.clearTimeout(timer);
      if (active) {
        if (!failed) setStatus("closed");
        terminal.writeln("\r\n\x1b[90m会话已关闭。\x1b[0m");
      }
    };
    session.current = {
      dispose: () => {
        active = false;
        ready = false;
        secret = "";
        window.clearTimeout(timer);
        socket.close();
        observer.disconnect();
        input.dispose();
        resize.dispose();
        terminal.dispose();
      },
      focus: () => terminal.focus(),
    };
  }

  return (
    <>
      <PageHeading
        eyebrow="Online operations"
        title="在线运维"
        description="通过 SSH 打开目标机器的真实 Bash 会话"
      />
      <section className="page-content operations-content">
        <div className="notice">
          终端命令会直接在选定的生产机器上执行，权限与登录账户一致。请确认目标后再输入命令。
        </div>
        {!enabled && <div className="notice">平台尚未启用在线运维。</div>}
        {targetsError && (
          <div className="notice notice--error" role="alert">
            {targetsError}
          </div>
        )}
        <form
          className="terminal-login"
          onSubmit={(event) => {
            event.preventDefault();
            connect();
          }}
        >
          <label>
            目标机器
            <select
              required
              value={nodeId}
              disabled={busy}
              onChange={(event) => {
                session.current?.dispose();
                session.current = null;
                setStatus("idle");
                setError("");
                setNodeId(event.target.value);
                setUsername(
                  targets.find((item) => item.node_id === event.target.value)
                    ?.default_user ?? "",
                );
                setPassword("");
              }}
            >
              <option value="" disabled>
                请选择目标机器
              </option>
              {targets.map((item) => (
                <option key={item.node_id} value={item.node_id}>
                  {nodes.find((node) => node.id === item.node_id)?.hostname ??
                    item.hostname}{" "}
                  · {item.host}:{item.port}
                </option>
              ))}
            </select>
          </label>
          <label>
            SSH 用户名
            <input
              required
              autoComplete="off"
              maxLength={128}
              value={username}
              disabled={busy}
              onChange={(event) => setUsername(event.target.value)}
            />
          </label>
          <label>
            SSH 密码
            <input
              required
              type="password"
              maxLength={4096}
              autoComplete="off"
              value={password}
              disabled={busy}
              onChange={(event) => setPassword(event.target.value)}
            />
          </label>
          {busy ? (
            <button
              type="button"
              className="button button--danger"
              onClick={() => {
                session.current?.dispose();
                session.current = null;
                setStatus("closed");
                setPassword("");
              }}
            >
              断开连接
            </button>
          ) : (
            <button
              type="submit"
              className="button"
              disabled={!enabled || !target || !username.trim() || !password}
            >
              连接终端 →
            </button>
          )}
        </form>
        <p className="hint">
          密码仅用于本次 SSH
          认证，提交后清空。切换页面或关闭浏览器将结束网页会话。平台不会自动输入任何运维命令。
        </p>
        {error && (
          <div className="notice notice--error" role="alert">
            {error}
          </div>
        )}
        {!targets.length && !targetsError && (
          <p className="hint">尚无配置了 SSH 连接的节点。</p>
        )}
        <div className="terminal-frame">
          <header>
            <span>
              <i
                className={
                  status === "connected"
                    ? "terminal-dot terminal-dot--online"
                    : "terminal-dot"
                }
              />
              {statusLabels[status]}
            </span>
            <span>
              {target ? `${username}@${target.host}` : "选择一台机器以开始"}
            </span>
            {status === "connected" && (
              <button
                type="button"
                className="text-button"
                onClick={() => session.current?.focus()}
              >
                聚焦终端
              </button>
            )}
          </header>
          <div
            className="terminal-container"
            ref={container}
            aria-label="SSH 终端"
          />
          {status === "idle" && (
            <div className="terminal-placeholder">
              <code>$_</code>
              <p>输入目标账户信息，建立加密会话。</p>
            </div>
          )}
        </div>
      </section>
    </>
  );
}
