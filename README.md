# EasyManage

裸机运维平台：Rust HTTPS Server、Rust Agent、PostgreSQL、Agent SQLite 离线队列与 React / TypeScript 控制台。平台可兼作节点，其他机器由 Agent 通过 HTTPS 上报。当前不支持 Docker，保留目录。

## 功能与权限

设备总览提供 CPU、内存、GPU 核心/显存、磁盘 I/O、网络的真实 10 分钟曲线及 CPU/GPU/SSD 温度、每日硬件详情。存储页提供只读挂载、分区拓扑和 SMR/CMR 信息；另有用户、网络、权限检查、Security、软件、代理、Gitea、AI 工具和编程环境页面。

平台页覆盖 Server 与全部 Agent 的 24 小时性能统计、每小时存储占用和可配置采集频率。Agent 管理提供可信候选更新、重启和应急强制重启。未知指标保持未知，快照保留实际采集时间。

Linux 系统级服务与 Full Agent 位于 `deploy/linux/system/`，普通用户 Server / Basic Agent 位于 `deploy/linux/user/`。用户模式并未废弃；Server 模式与 Agent 权限独立，兼容现有混合部署，不自动迁移或降权。

内网只读页面无平台登录。在线终端、提权、账户操作和强制重启需目标主机实际 SSH / sudo 凭据；Full 状态以 root / UID 0 的新心跳确认。普通用户不可见的进程、权限或硬件信息标为未知。

## 构建与部署

构建不启动服务。已配置私有清单及可信目标的平台会发布 Agent 候选包，但不会自动更新运行中的 Agent：

```bash
./scripts/run-server.sh --build
```

公开清单 `deploy/examples/fleet.example.json` 只有示例地址，不能直接执行。实际配置放 Git 忽略的 `deploy/local/fleet.json`，已有节点保留原 ID 与 SQLite 状态。SSH 密钥须核验；自定义清单可用 `--inventory`。

数据库支持通用 PostgreSQL 本地/远程配置，使用 `DATABASE_URL` 或 `PGHOST/PGPORT/PGDATABASE/PGUSER/PGPASSWORD/PGSSLMODE`，不固定本机地址、端口或系统用户。模板位于 `server/config/server.env.example`、`agent/config/agent.env.example`；真实配置存仓库外并设为 0600。Server 自动执行版本化迁移。

详见 [部署](deploy/README.md)、[数据库](deploy/database/README.md)、[系统安装](deploy/linux/system/README.md)、[用户模式](deploy/linux/user/README.md) 和 [脚本](scripts/README.md)。已有生产服务不因目录整理切换运行模式，已提权 Agent 保持 Full。

## 目录约定

| 目录 | 职责 |
| --- | --- |
| `server/`、`agent/` | 平台 API / 运维、主机采集 |
| `agent/src/platform/` | 统一采集接口、可选共用探测工具及 Linux 原生后端 |
| `protocol/`、`runtime/` | 共享协议、进程与存储观测、权限策略 |
| `web/`、`docs/` | 前端、VitePress 操作手册与平台文档 |
| `migrations/` | 已发布迁移不修改，新增独立序号 |
| `deploy/linux/` | system / user 安装、bootstrap 准备、operations 运维、migrations 模式交接 |
| `deploy/orchestration/`、`deploy/examples/` | 可信编排（目前仅 Linux）、公开清单模板 |
| `deploy/windows/`、`deploy/macos/` | 原生部署接入说明，尚无安装器 |
| `scripts/diagnostics/` | 只读诊断 |
| `tests/integration/`、`web/tests/` | 隔离回归、前端测试 |
| `docs/architecture.md`、`docs/design/` | 当前架构、跨平台接入约定与历史设计 |
| `deploy/docker/` | 按要求保留，当前不支持容器 |

Git 追踪源码、迁移、配置模板、测试、文档、锁文件及部署脚本；不追踪凭据、本地连接文档、生产清单、运行数据库、依赖目录或构建产物。外部完整项目放忽略的 `third_party/` 或 `vendor_projects/`。目录与测试约定见 [tests](tests/README.md)。

Agent 真实入口见 [agent/README.md](agent/README.md)。当前只有 Linux 后端；Windows/macOS 接入方式见 [架构说明](docs/architecture.md)。没有创建空平台模块或假采集实现；原 `agent/crates/`、`server/crates/`、`agent/packaging/`、前端空 feature/state 及未接入的 C++ 模板已清理。历史需求迁入 `docs/design/Requirement.md`，不作为当前实现保证。

## 性能与安全

Agent 使用两个异步工作线程，网卡速率保持实时采样，网卡列表按扩展清单周期发现变化。慢速清单和权限检查单任务后台刷新，保留上一次真实观测，不为等待扫描阻塞快采样。GPU 失败退避至少 60 秒；存储占用每小时、硬件详情每日采集。SQLite 仍使用单连接 WAL 与有界离线队列，更新不丢失身份。

两台既有主机均为生产环境。编译、静态检查可在工作区执行；运行时、故障、提权和压力测试只在隔离环境执行。部署、迁移、轮换密钥及强制重启是真实风险操作，应先确认目标及维护窗口。
