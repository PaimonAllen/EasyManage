import { lazy, Suspense, useEffect, useState } from "react";
import { fetchPlatform, fetchTargets } from "../api/client";
import type { OperationTarget, PlatformConfig } from "../api/client";
import { ElevationDialog } from "../components/ElevationDialog";
import { AgentManagement } from "../pages/AgentManagement";
import { AiTools } from "../pages/AiTools";
import { ProgrammingEnvironment } from "../pages/ProgrammingEnvironment";
import { Gitea } from "../pages/Gitea";
import { GlobalNetwork } from "../pages/GlobalNetwork";
import { OSandSoftwares } from "../pages/OSandSoftwares";
import { PlatformHealth } from "../pages/PlatformHealth";
import { PermissionChecks } from "../pages/PermissionChecks";
import { Security } from "../pages/Security";
import { useFleet } from "../hooks/useFleet";
import { Dashboard } from "../pages/Dashboard";
// import { Documentation } from "../pages/Documentation";
import { Network } from "../pages/Network";
import { Storage } from "../pages/Storage";
import { Users } from "../pages/Users";
import type { NodeSummary } from "../types/monitoring";

const Operations = lazy(() => import("../pages/Operations"));
const GpuComparison = lazy(() => import("../pages/GpuComparison"));
type Theme = "dark" | "light";
const THEME_KEY = "easymanage-theme";
const DOC_THEME_KEY = "vitepress-theme-appearance";

function savedTheme(): Theme {
  try {
    const value = localStorage.getItem(THEME_KEY) ?? localStorage.getItem(DOC_THEME_KEY);
    return value === "light" ? "light" : "dark";
  } catch {
    return "dark";
  }
}

const pages = [
  // { id: "docs", label: "操作手册" },
  { id: "overview", label: "设备总览" },
  { id: "users", label: "用户状态" },
  { id: "storage", label: "存储" },
  { id: "network", label: "网络状态" },
  { id: "operations", label: "在线运维" },
  { id: "security", label: "Security" },
  { id: "permissions", label: "权限检查" },
  { id: "platform-health", label: "平台" },
  { id: "software", label: "OSandSoftwares" },
  { id: "global-network", label: "Global Network" },
  { id: "gitea", label: "Gitea 状态" },
  { id: "ai-tools", label: "AI 工具状态" },
  { id: "programming-environment", label: "编程环境" },
  { id: "gpu-comparison", label: "GPU 性能对比" },
  { id: "agent-management", label: "Agent 管理" },
] as const;

function currentPage() {
  const hash = window.location.hash.replace(/^#\/?/, "");
  if (hash === "agent-updates") return "agent-management";
  return pages.some((page) => page.id === hash) ? hash : "overview";
}

export function App() {
  const [page, setPage] = useState(currentPage);
  const [theme, setTheme] = useState<Theme>(savedTheme);
  const documentPage = page === "gpu-comparison";
  const needsTargets = ["overview", "operations", "agent-management"].includes(page);
  const needsFleet = page !== "platform-health" && page !== "gitea" && !documentPage;
  const fleet = useFleet(page === "overview" ? 10_000 : 30_000, needsFleet, page === "software");
  const [config, setConfig] = useState<PlatformConfig>({
    documentation_url: "/guide/easymanage.html",
    operations_enabled: false,
  });
  const [targets, setTargets] = useState<OperationTarget[]>([]);
  const [targetsError, setTargetsError] = useState("");
  const [configError, setConfigError] = useState("");
  const [elevating, setElevating] = useState<NodeSummary | null>(null);
  const [documentDirty, setDocumentDirty] = useState(false);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.querySelector('meta[name="theme-color"]')?.setAttribute("content", theme === "light" ? "#f4f7fa" : "#090c10");
    try {
      localStorage.setItem(THEME_KEY, theme);
      localStorage.setItem(DOC_THEME_KEY, theme);
    } catch {
      // The theme still works for this tab if browser storage is unavailable.
    }
  }, [theme]);

  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if ((event.key === THEME_KEY || event.key === DOC_THEME_KEY) && (event.newValue === "light" || event.newValue === "dark")) {
        setTheme(event.newValue);
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  useEffect(() => {
    const onHashChange = () => {
      const nextPage = currentPage();
      if (documentDirty && nextPage !== "gpu-comparison") {
        if (!window.confirm("GPU 文档有未保存的修改，离开会丢弃草稿。确认离开？可先下载草稿。")) {
          window.history.replaceState(null, "", "#/gpu-comparison");
          return;
        }
        setDocumentDirty(false);
      }
      setPage(nextPage);
      window.scrollTo(0, 0);
    };
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, [documentDirty]);

  useEffect(() => {
    const controller = new AbortController();
    void fetchPlatform(controller.signal)
      .then(setConfig)
      .catch((caught: unknown) => {
        if (!controller.signal.aborted)
          setConfigError(
            caught instanceof Error ? caught.message : "平台配置读取失败",
          );
      });
    return () => controller.abort();
  }, []);

  useEffect(() => {
    if (!needsTargets) return;
    const controller = new AbortController();
    let pending = false;
    async function refreshTargets() {
      if (pending || document.hidden) return;
      pending = true;
      try {
        setTargets(await fetchTargets(controller.signal));
        setTargetsError("");
      } catch (caught) {
        if (!controller.signal.aborted)
          setTargetsError(
            caught instanceof Error ? caught.message : "SSH 目标配置读取失败",
          );
      } finally {
        pending = false;
      }
    }
    void refreshTargets();
    const timer = window.setInterval(() => void refreshTargets(), 60_000);
    document.addEventListener("visibilitychange", refreshTargets);
    return () => {
      controller.abort();
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", refreshTargets);
    };
  }, [needsTargets]);

  const nodes = fleet.data?.nodes ?? [];
  return (
    <div className="dashboard">
      <header className="topbar">
        <a className="brand" href="#/overview">
          <span>E</span>
          <strong>EasyManage</strong>
        </a>
        <div className="topbar__right">
          <span className="network-scope">内网基础设施平台</span>
          <span
            className={`live-indicator ${fleet.error ? "live-indicator--error" : ""}`}
          >
            <i />
            {documentPage ? "文档按需读取" : fleet.error ? "采集接口连接异常" : page === "overview" ? "总览实时指标每 1 秒刷新" : "每 30 秒刷新"}
          </span>
          <button
            className="theme-toggle"
            type="button"
            onClick={() => setTheme(theme === "dark" ? "light" : "dark")}
            aria-label={`切换到${theme === "dark" ? "浅色" : "深色"}主题`}
            title={`切换到${theme === "dark" ? "浅色" : "深色"}主题`}
          >
            <span aria-hidden="true">{theme === "dark" ? "☀" : "☾"}</span>
            <span>{theme === "dark" ? "浅色" : "深色"}</span>
          </button>
          <a
            className="github-link"
            href="https://github.com/PaimonAllen/EasyManage"
            target="_blank"
            rel="noopener noreferrer"
            aria-label="EasyManage GitHub 仓库（新标签页打开）"
            title="EasyManage · GitHub"
          >
            <svg viewBox="0 0 16 16" width="22" height="22" fill="currentColor" aria-hidden="true" focusable="false">
              <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0 0 16 8c0-4.42-3.58-8-8-8Z" />
            </svg>
          </a>
        </div>
      </header>
      <nav className="primary-nav" aria-label="主导航">
        <div>
          {pages.map((item) => (
            <a
              key={item.id}
              href={`#/${item.id}`}
              aria-current={page === item.id ? "page" : undefined}
            >
              {item.label}
            </a>
          ))}
        </div>
      </nav>
      <main>
        {needsFleet && fleet.error && (
          <div className="alert global-alert" role="alert">
            {fleet.error}
            {fleet.data && " · 当前保留上一次数据"}
            <button onClick={() => void fleet.refresh()}>重试</button>
          </div>
        )}
        {page !== "docs" && configError && (
          <div className="alert global-alert" role="alert">
            平台配置：{configError}
          </div>
        )}
        {/* {page === "docs" && (
          <Documentation
            url={config.documentation_url}
          />
        )} */}
        {page === "overview" && (
          <Dashboard
            data={fleet.data}
            targets={targets}
            onElevate={setElevating}
            onRefresh={fleet.refresh}
          />
        )}
        {page === "users" && <Users nodes={nodes} targets={targets} />}
        {page === "storage" && <Storage nodes={nodes} />}
        {page === "network" && <Network nodes={nodes} />}
        {page === "security" && <Security nodes={nodes} />}
        {page === "permissions" && <PermissionChecks nodes={nodes} />}
        {page === "platform-health" && <PlatformHealth />}
        {page === "software" && <OSandSoftwares nodes={nodes} />}
        {page === "global-network" && <GlobalNetwork nodes={nodes} />}
        {page === "gitea" && <Gitea />}
        {page === "ai-tools" && <AiTools nodes={nodes} />}
        {page === "programming-environment" && <ProgrammingEnvironment nodes={nodes} />}
        {documentPage && <Suspense fallback={<div className="loading">正在载入 GPU 文档…</div>}><GpuComparison onDirtyChange={setDocumentDirty} /></Suspense>}
        {page === "agent-management" && <AgentManagement nodes={nodes} targets={targets} onRefresh={() => void fleet.refresh()} onElevate={setElevating} />}
        {page === "operations" && (
          <Suspense fallback={<div className="loading">正在载入终端…</div>}>
            <Operations
              nodes={nodes}
              targets={targets}
              enabled={config.operations_enabled}
              targetsError={targetsError}
            />
          </Suspense>
        )}
        {needsFleet &&
          page !== "operations" &&
          page !== "agent-management" &&
          fleet.loading &&
          !fleet.data && (
            <div className="loading">
              <span />
              <p>正在读取节点指标…</p>
            </div>
          )}
        {needsFleet &&
          page !== "operations" &&
          page !== "agent-management" &&
          fleet.data &&
          nodes.length === 0 && (
            <div className="empty-dashboard">
              <h2>尚无受管节点</h2>
              <p>Agent 注册并发送第一组指标后会自动出现在这里。</p>
            </div>
          )}
        {page === "overview" && targetsError && (
          <div className="alert" role="alert">
            SSH 目标配置：{targetsError} · 暂不能从页面提权
          </div>
        )}
      </main>
      <footer className="app-footer">
        <span>EasyManage · Production infrastructure</span>
        <span>指标按节点实际采集时间显示</span>
      </footer>
      {elevating && (
        <ElevationDialog
          node={elevating}
          defaultUser={
            targets.find((target) => target.node_id === elevating.id)
              ?.default_user
          }
          onClose={() => setElevating(null)}
          onSubmitted={() => void fleet.refresh()}
        />
      )}
    </div>
  );
}
