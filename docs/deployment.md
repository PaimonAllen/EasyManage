# 裸机部署

## 更新平台本身

「Agent 管理 → 平台自身更新」可安装已发布的后端、前端和手册候选，并只重启平台服务，不更改 Agent 身份或 Full / Basic 权限。先以平台普通用户执行 `./scripts/run-server.sh --build`，或对已有构建执行 `--publish-platform`。网页不自动构建或拉取 Git，未发布/已是候选版本时按钮禁用；第一次使用需先手动部署含此功能的平台版本。

点击更新需同源 HTTPS、可信本机 SSH 主机密钥、管理员 SSH/sudo 验证，并确认短暂离线与数据库备份。平台使用独立任务安装并核验 HTTPS/数据库就绪，页面展示结果，成功后可载入新版；失败尝试回退平台文件，**已执行的数据库迁移不会回退**。标准 Linux 普通用户布局下，保留平台自身目录、清单及候选文件的权限检查，但允许 home、`.local` 等上级目录组写入，不再因此禁用添加机器或平台更新。该内网兼容策略不防范同机组成员替换上级目录，不自动修改生产权限。详见源码中的 `deploy/linux/operations/README.md`。

## 从设备总览添加机器

设备总览的“机器管理”支持添加、编辑显示名称/备注、软删除与恢复。新增需输入内网 IP、SSH 端口、目标可访问的本平台 HTTPS 地址，选择密码或 SSH 私钥鉴权，并独立核验首次 SSH 主机指纹。私钥上传支持 OpenSSH / PEM，最大 12 KiB，可单独输入私钥解密口令；推荐带口令的私钥，不接受 `.pub` 公钥或 PuTTY `.ppk`。凭据只用于此次添加，默认安装 Basic Agent。当前仅支持 Linux、`/home/账户`、与最新已发布 ELF 候选匹配的架构。目标需 Python3、systemd 用户 manager 与预先启用的 linger；不会自动执行 sudo。已有 Agent 配置/状态拒绝覆盖，应接管或恢复原机器，不重新生成身份。

平台服务用户需有安全可写的可信清单目录，配置 `EASYMANAGE_OPERATIONS_TARGETS`。浏览器管理的增量存 `managed-targets.json`，不会覆盖原 fleet 清单；每机器已确认的 SSH 公钥存同目录独立 `.known_hosts` 文件，均不入 Git。设置 `EASYMANAGE_AGENT_CA_CERTIFICATE` 指向 Agent 信任的 CA（默认平台证书同目录 `ca.crt`），候选默认来自平台用户已发布的 `~/.local/share/easymanage/releases/current/target/release/easymanage-agent`。系统模式平台可用 `EASYMANAGE_AGENT_RELEASE_ROOT` 指定经批准的发布目录。 目前页面不改 SSH 地址/用户名；新机器未配置 SSH 免密分发时，现有依赖免密的 Basic 更新/重启功能不会冒充可用，后续仍可用管理员凭据提权。

部署任务显示阶段、百分比和受控日志，上传、SHA-256 校验、服务启动后必须确认真实 Basic 心跳才标成功。SSH 密码、私钥及私钥口令均不保存到数据库、配置、浏览器存储或任务日志；输入框提交/切换鉴权方式时清空，不展示私钥正文。浏览器到平台通过 HTTPS 加密，平台到目标通过 SSH 加密；必须验证平台证书和目标指纹，忽略证书告警会削弱防中间人保护。平台使用内存私钥 API 签名，不创建临时私钥文件、不把私钥作为进程参数、不把私钥下发给 Agent。鉴权后清理 Rust 自有凭据字符串；浏览器、TLS/JSON 解析器及 SSH 库的内部内存副本不能保证全部擦除，程序工作时必须短暂解密，**不承诺全程内存密文或防御已被控制的浏览器/操作系统**。错误响应和日志使用固定脱敏消息，不回显请求中的凭据或 SSH 原始错误。凭据不持久化，因此该私钥不会自动成为后续更新/重启的免密凭据。

新机器仅获得单机、10 分钟、一次性注册凭据。任务能力凭据只留当前页面内存，日志最多保留 30 分钟；关闭页面不取消服务端任务。失败保留机器 ID 和必要的私有暂存包，可能已经创建用户服务，不能盲目重新安装新身份。

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

如需完整账户和状态，由 **Gitea 管理员**在 Gitea 网页的用户设置 → 应用中创建仅含 `read:admin` 范围的访问令牌。将令牌单独存入平台运行用户独占、权限为 `0600` 的绝对路径文件（例如 `/home/tzr/.local/share/easymanage/config/gitea-admin-read.token`），在平台的 `server.env` 中加入 `EASYMANAGE_GITEA_ADMIN_TOKEN_FILE=该绝对路径`，并在维护窗口重启平台服务。不要把令牌写入仓库、浏览器或命令行参数。平台只向本机 Gitea 发起 GET 请求，不向网页返回令牌或邮箱。

`read:admin` 可读取 Gitea 其他管理员 API 数据，属于敏感凭据；应只授予可信任的平台运行用户，并按需轮换。平台目前没有登录鉴权，完整账户清单会对可访问该内网页面的人可见。若不接受这一范围，请保持未配置状态。账户「正常」仅根据 Gitea 的激活、禁止登录和受限标志判断，不验证密码、SSH Key 或仓库权限。
