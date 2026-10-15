# 当前架构与跨平台接入

本次整理保持 Linux 现有功能与生产路径，不更新线上服务、协议版本或数据库迁移。Windows/macOS 是接入目标，尚未实现；Server 在线 SSH Bash、systemd 提权及服务管理也仍是 Linux 功能。

## 仓库职责

| 目录 | 当前职责 |
| --- | --- |
| `agent/` | 单 crate Agent：共用运行层和原生采集后端 |
| `server/` | HTTPS/API、PostgreSQL、只读观测与受认证的运维操作 |
| `protocol/` | OS 无关的消息和采集策略；原字段兼容 |
| `runtime/` | 共用进程观测接口、文件计量、权限策略与优雅退出 |
| `web/` | 真实页面、组件、hooks 和展示逻辑，不保留空 feature/state |
| `migrations/` | 只追加迁移，已发布文件保持原字节/校验和 |
| `deploy/linux/system/`、`deploy/linux/user/` | 系统级与普通用户服务单元、安装器；与 Agent 权限独立 |
| `deploy/linux/bootstrap/`、`operations/`、`migrations/` | 环境准备、固定运维操作、显式部署模式交接 |
| `deploy/orchestration/`、`deploy/examples/` | 可信发布/分发（目前仅 Linux）与公开清单模板 |
| `deploy/windows/`、`deploy/macos/` | 带接入说明的准备目录，尚无安装器 |
| `deploy/database/` | 通用本地/远程 PostgreSQL |
| `scripts/` | 构建、运行、规范检查和只读诊断 |
| `tests/integration/`、`web/tests/` | 隔离集成和前端回归 |
| `docs/design/` | 历史设计，不打包进在线手册 |
| `deploy/docker/` | 仅按要求保留，不支持容器 |

无实现的旧 crates、重复 packaging、C++ interface 模板和前端占位已移除。Windows/macOS 部署目录仅保留原生接入清单，不创建假安装器或空子模块。第三方、生产连接资料、配置、身份库、缓存及构建目录是忽略的本机数据，不做清理。

## Agent 边界

`main.rs` 仅处理启动，`app.rs` 共用采集策略、注册、轻量上报和有界离线发送。HTTPS 客户端、SQLite Store 与配置校验只保留一份。

`platform/contract.rs` 定义 `PlatformCollector`：初始化、应用采集策略、节点信息、轻量采集、完整快照。`platform/mod.rs` 通过目标 OS 选择实现；目前选择 Linux。接口使用泛型静态调用，不引入动态分发、额外线程或另一个运行时。

Linux 的 `/proc`、`/sys`、UID/GID、ACL、systemd、lsblk、ip、NSS 及目录权限规则只在 `platform/linux/`。Linux 的采集缓存也留在该后端，避免把当前路径/设备名规则伪装成各 OS 通用实现。共用固定命令执行器和可选 NVIDIA 查询在 `platform/common/`，新平台只有具备工具时才启用。

配置仍接受显式 `EASYMANAGE_ENV_FILE`、`EASYMANAGE_AGENT_STATE_DB`。Linux 的 root 默认 `/var/lib/easymanage-agent/state.db`，普通用户默认 XDG/HOME；服务配置里已有的路径继续优先。不可因源码移动重建节点 UUID、token 或硬件缓存。

Runtime 的公开 `process::ProcessSampler` 路径保持不变；`/proc` 实现在 `runtime/src/platform/linux/`。其他 OS 尚无原生进程计量时返回未知，而不读取 Linux 伪路径或补零；这不表示已支持该 OS 的完整 Agent。文件计量与 Ctrl-C/SIGTERM 处理可共用。

## Windows/macOS 的接入步骤

1. 在有实际实现时创建 `agent/src/platform/windows/` 或 `macos/`，实现采集接口，不复制调度、HTTP 和队列。Windows 使用原生账户、卷、服务及传感器接口；macOS 使用原生设备和系统接口，不调用 Linux 专用工具。
2. 实现该 OS 的配置/状态位置、文件保护与管理员判断。Windows 管理员不是 UID 0，不能拿 Linux 的 UID 判定冒充 Full。现有协议/前端仍有 UID/GID 和 systemd 语义，应新增向后兼容的原生身份、权限和能力字段并完成联动测试后再启用。
3. 没有传感器、GPU CLI、文件系统或权限指标时显示未知/不支持；权限不足不能判定正常。Linux 的 sudo/root/共享目录规范不可原样套到 Windows 或 macOS。
4. 在 `platform/mod.rs` 和启动 target guard 注册后端。为新平台补充自有进程计量和优雅退出测试，核验第三方依赖与工具链的目标支持。
5. 创建实际安装器：Windows 服务、macOS launchd、Linux systemd；安装资产进入相应 `deploy/` 目录，编排按目标能力分发。绝不向 Windows/macOS 自动执行 Linux 的 sudo、Bash 或 systemd 脚本。Basic/Full 已有权限不在更新时自动改变。
6. 在各 OS 的隔离环境验证：真实采集、缺失指标、管理员/普通用户、注册、证书、离线/恢复、重启身份、更新和跨版本报告；未验证前不列为支持平台。

## 验证与 Git

`scripts/check-repository.py --staged` 检查源码、锁文件、原生后端、Docker 保留目录及迁移完整性，拒绝私有/生成文件和旧空模块占位。运行时测试在隔离命名空间或相应 OS 的测试机/VM 中进行，不在生产服务器测试修改服务或权限。
