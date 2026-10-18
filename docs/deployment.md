# 裸机部署

## 更新平台本身

「Agent 管理 → 平台自身更新」可安装已发布的后端、前端和手册候选，并只重启平台服务，不更改 Agent 身份或 Full / Basic 权限。先以平台普通用户执行 `./scripts/run-server.sh --build`，或对已有构建执行 `--publish-platform`。网页不自动构建或拉取 Git，未发布/已是候选版本时按钮禁用；第一次使用需先手动部署含此功能的平台版本。

点击更新需同源 HTTPS、可信本机 SSH 主机密钥、管理员 SSH/sudo 验证，并确认短暂离线与数据库备份。平台使用独立任务安装并核验 HTTPS/数据库就绪，页面展示结果，成功后可载入新版；失败尝试回退平台文件，**已执行的数据库迁移不会回退**。标准 Linux 普通用户布局下，保留平台自身目录、清单及候选文件的权限检查，但允许 home、`.local` 等上级目录组写入，不再因此禁用添加机器或平台更新。该内网兼容策略不防范同机组成员替换上级目录，不自动修改生产权限。详见源码中的 `deploy/linux/operations/README.md`。

## 从设备总览添加机器

设备总览的“机器管理”支持添加、编辑显示名称/备注、软删除与恢复。新机器无需预装 Agent：填写内网 IP、SSH 端口和用户名，选择运行权限及密码或 SSH 私钥鉴权，并独立核验首次 SSH 主机指纹；确认后平台通过 SSH 下发最新已发布 Agent、安装并启动，页面展示部署进度与日志，并等待对应权限的首次心跳。Agent 的平台连接地址默认使用当前页面地址，自动写入配置；只有目标机器无法访问该地址时，才需展开“高级网络设置”修改。私钥支持 OpenSSH / PEM（最大 12 KiB）及解密口令，不接受 `.pub` 或 `.ppk`。目标需 Linux、Python3、systemd 及与 ELF 候选匹配的架构。已有 Agent 配置/状态拒绝覆盖，应接管或恢复原机器，不重新生成身份。

- **普通用户 Basic（默认）**：SSH 账户必须是非 root 且 home 为 `/home/账户`，只要求 Linux、Python3 和匹配的 CPU 架构，不再要求预先启用 linger。有可用用户 manager 且 linger 已启用时优先安装 systemd 用户服务；否则自动安装普通用户兼容守护进程。程序、配置和 SQLite 状态库统一位于 `~/.easymanage/`，临时安装包位于其中的 `staging/agent.XXXXXXXX`；使用 systemd 时单元仍在标准的 `~/.config/systemd/user/`。不调用 sudo、不写系统安装目录、不主动修改 linger/logind；后续可从设备总览提权为 Full。
- **管理员 Full（首次直接安装）**：支持 SSH root（home 为 `/root`）或有 sudo 权限的普通 SSH 账户；不要求用户 manager / linger，不先运行 Basic。密码登录时 sudo 密码留空默认复用 SSH 密码；密钥登录可另填 sudo 密码，免密 sudo 可留空。固定安装器经管理员验证后，将程序安装到 `/usr/local/sbin/easymanage-agent`、配置到 `/etc/easymanage/`、状态到 `/var/lib/easymanage-agent/`，创建 root 系统服务并启用开机启动。安装前重新检查已有身份，校验二进制/CA/配置摘要；服务启动失败时尝试撤回本次新增服务和文件并保留恢复数据，不承诺网络中断后的完整回滚，不重启整机。

### 无 linger 的普通用户兼容模式

`~/.easymanage/bin/agent-user-control.py` 以 SSH 用户运行独立后台 Python 守护进程，启动并在崩溃后重启 Agent。后台进程关闭 SSH 的标准输入/输出并建立独立会话；锁由 Agent 一并持有，避免守护进程意外退出后产生重复采集器。日志在 `~/.easymanage/logs/agent.log`，最多保留两个各 1 MiB 的文件。该模式额外常驻一个 Python 进程，不视为系统服务。

若允许用户 crontab，安装器仅新增/替换 `EasyManage Agent compatibility` 标记块，保留其他用户任务；通过 `@reboot` 与每分钟检查恢复守护。不开启或配置系统 cron 服务，是否执行依赖目标机 cron 实际运行及账户策略。无法读取 crontab、禁止使用或没有命令时，不阻断首次安装，任务日志会明确提示无法保证开机/会话退出后的自动恢复。禁止把这种情况显示为完整开机自启保证。[logind 的会话清理策略](https://github.com/systemd/systemd/blob/main/man/logind.conf.xml) 仍可能回收后台进程，普通用户模式不会绕过管理员限制。

手动控制：`python3 ~/.easymanage/bin/agent-user-control.py restart`（或 `status`、`stop`）。`stop` 会禁用恢复标记，cron 不会擅自重新启动；再次显式 `start` 可恢复。提权会先以原用户停止兼容守护并禁用恢复标记，确认退出后才复制 SQLite 身份；Full 启动后不会被 cron 拉回 Basic。提权失败时尝试恢复此前启用的兼容实例。

平台服务用户需有安全可写的可信清单目录，配置 `EASYMANAGE_OPERATIONS_TARGETS`。浏览器管理的增量存 `managed-targets.json`，不会覆盖原 fleet 清单；每机器已确认的 SSH 公钥存同目录独立 `.known_hosts` 文件，均不入 Git。设置 `EASYMANAGE_AGENT_CA_CERTIFICATE` 指向 Agent 信任的 CA（默认平台证书同目录 `ca.crt`），候选默认来自平台用户已发布的 `~/.easymanage/releases/current/target/release/easymanage-agent`。系统模式平台可用 `EASYMANAGE_AGENT_RELEASE_ROOT` 指定经批准的发布目录。 目前页面不改 SSH 地址/用户名；新机器未配置 SSH 免密分发时，现有依赖免密的 Basic 更新/重启功能不会冒充可用，后续仍可用管理员凭据提权。

部署任务显示阶段、百分比和受控日志，上传、SHA-256 校验、服务启动后必须确认真实 Basic / SSH 用户 UID 或 Full / UID 0 心跳才标成功。SSH / sudo 密码、私钥及私钥口令均不保存到数据库、配置、浏览器存储或任务日志；输入框提交/切换模式时清空，不展示私钥正文。浏览器到平台通过 HTTPS 加密，平台到目标通过 SSH 加密；必须验证平台证书和目标指纹，忽略证书告警会削弱防中间人保护。平台使用内存私钥 API 签名，不创建临时私钥文件、不把私钥或 sudo 密码作为进程参数、不把 SSH 凭据下发给 Agent。SSH 鉴权后清理 Rust 自有 SSH 凭据字符串，管理员安装结束后清理 sudo 口令；浏览器、TLS/JSON 解析器及 SSH 库的内部内存副本不能保证全部擦除，程序工作时必须短暂解密，**不承诺全程内存密文或防御已被控制的浏览器/操作系统**。错误响应和日志使用固定脱敏消息，不回显请求中的凭据或 SSH 原始错误。凭据不持久化，因此该私钥不会自动成为后续更新/重启的免密凭据。

新机器仅获得单机、10 分钟、一次性注册凭据。任务能力凭据只留当前页面内存，日志最多保留 30 分钟；关闭页面不取消服务端任务。失败保留机器 ID 和必要的私有暂存包，可能已经创建用户/系统服务，不能盲目重新安装新身份。

软删除保留历史指标、机器 UUID、Agent 凭据哈希和远端文件，不卸载服务、不删除身份库。平台停用上报凭据、拒绝该 UUID 重新注册并从当前监控隐藏；恢复重启用原凭据，等待新心跳。历史仍受正常保留周期管理。改/删/恢复需该机器登记用户或管理员 SSH 鉴权；目标离线可通过平台本机可信管理员 SSH/sudo 鉴权。平台本机节点有删除保护。编辑名称不会被后续 Agent 原始主机名上报覆盖。

当前只支持 Linux 裸机部署，不使用 Docker。系统级服务位于 `deploy/linux/system/`，普通用户部署位于 `deploy/linux/user/`，两种模式均有效。Windows/macOS 准备目录只放接入说明，尚无可运行安装器。部署前核对服务状态、目标用户、SSH 主机密钥和磁盘空间；目录整理不自动迁移现有服务。

以平台普通 SSH 用户在仓库根目录构建：

```bash
./scripts/run-server.sh --build
```

该命令构建 Server、Agent、React 控制台和 VitePress 文档；不会立即重启运行中的服务。已初始化的平台还会发布供网页分发的 Agent 候选版本。

已有普通用户部署可按需执行以下命令（不是系统级安装命令）：

```bash
./scripts/run-server.sh --fleet
./scripts/run-server.sh --status
```

`--fleet` 会更新并短暂重启用户 Server 与 Basic Agent，已启用的 Full 保持管理员权限边界。数据库使用通用 PostgreSQL 的 `DATABASE_URL` 或 PG* 配置，本地/远程、端口及账户均可指定；模板位于 `server/config/server.env.example`，见 `deploy/database/README.md`。系统级安装见 `deploy/linux/system/README.md`，用户模式迁移见 `deploy/linux/user/README.md`，均需明确操作。

## Gitea 账户只读观测

「Gitea 状态」默认通过本机公开 API 列出公开可见账户；公开接口可能隐藏账户，并可能遮蔽账户状态，因此页面统一标记为「待核验」，不能据此判断账户是否正常。

如需完整账户和状态，由 **Gitea 管理员**在 Gitea 网页的用户设置 → 应用中创建仅含 `read:admin` 范围的访问令牌。将令牌单独存入平台运行用户独占、权限为 `0600` 的绝对路径文件（例如 `/home/tzr/.easymanage/config/gitea-admin-read.token`），在平台的 `server.env` 中加入 `EASYMANAGE_GITEA_ADMIN_TOKEN_FILE=该绝对路径`，并在维护窗口重启平台服务。不要把令牌写入仓库、浏览器或命令行参数。平台只向本机 Gitea 发起 GET 请求，不向网页返回令牌或邮箱。

`read:admin` 可读取 Gitea 其他管理员 API 数据，属于敏感凭据；应只授予可信任的平台运行用户，并按需轮换。平台目前没有登录鉴权，完整账户清单会对可访问该内网页面的人可见。若不接受这一范围，请保持未配置状态。账户「正常」仅根据 Gitea 的激活、禁止登录和受限标志判断，不验证密码、SSH Key 或仓库权限。
