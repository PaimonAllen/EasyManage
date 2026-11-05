# Linux 运维操作

这里放平台调用的固定受认证操作脚本；常规安装入口仍位于 `system/` 和 `user/`。

- `promote-agent.sh`：受认证的 Basic → Full 提权，保留节点身份与回滚边界。SQLite 主文件及 WAL / SHM 单文件上限为 256 MiB；既有 Full 使用下述二进制更新器。
- `install-new-full-agent.sh`：GUI 首次安装 Full 的内嵌固定安装器，仅接受新机器；root/sudo 执行前验证摘要与无既有身份，不依赖用户 linger，不运行普通用户暂存目录里的脚本。失败尝试撤回新增服务并保留恢复数据。
- `install-full-candidate.sh`：只暂存已验证的 Full 候选包，不由普通用户改变运行中的 root 服务。
- `restart-full-agent.sh`、`force-restart-service.sh`：受认证的 Agent / 平台重启；不是整机重启。
- `update-full-over-ssh.sh`：直连和 SSH-only 共用的 Full 更新器。只替换已核验的 Agent 二进制并重启原系统服务，不重新提权安装，不复制或重写 SQLite、配置与服务单元，因此较大的身份 / 待上报数据库不会触发提权安装的文件大小限制。首次 Basic → Full 仍使用 `promote-agent.sh`。
- `manage-user.sh`：受管理员认证的目标账户操作。
- `platform-release.py`：平台自身候选发布、完整性检查、独立更新与文件回退；不接触 Agent、私有配置及数据库文件。
- `renew_controller_tls_for_interfaces.sh --apply`：复用现有 CA 重签用户平台证书，会短暂重启平台，需显式指定服务用户。

这些脚本由平台认证流程或准确的暂存包调用，不是无凭据的公共 CLI。提权、更新、账户及证书修改会影响生产状态；不要把目录整理当作执行授权。Full 不因用户部署或暂时离线降回 Basic。

## 平台自身更新

`stop-agent-service.py` 是固定 Agent 停服器，由同源网页鉴权后内嵌调用；不提供任意服务名、路径或启动操作。核验 UUID 与 Basic / Full 身份后，关闭进程和自动拉起并保留文件与数据库。完全停止后必须重新部署，不能用恢复监控或强制重启绕过；暂停监控仍保留轻量心跳，可直接恢复。不要在未指定生产目标并确认停服影响时直接执行该脚本。

仅支持标准 Linux 普通用户平台服务：`~/.easymanage/bin/easymanage-server`、`web`、`config/server.env`，以及可信清单中 `ssh_target=local` 的平台用户。其他布局保持只读提示，不猜测服务位置。

以平台用户执行 `./scripts/run-server.sh --build` 完成构建并发布候选；已有产物也可用 `./scripts/run-server.sh --publish-platform` 只发布。发布不重启、不修改运行中的平台或 Agent。候选保存在用户私有 `platform-releases/<UUID>` 中，清单包含所有文件的 SHA-256；发布索引原子切换，浏览器确认时绑定摘要，安装前重新逐文件核验。平台自身目录、候选目录和文件保持私有，拒绝符号链接和不可信所有者；home、`.local` 等上级目录允许组写入，不因此阻止操作，也不自动 chmod。这是内网部署的兼容取舍：不再防范受信任的同机组成员替换上级目录。机器管理同样只要求实际清单目录由平台用户安全写入，不因上级目录组写入而禁用。平台候选发布失败不阻断原有 Agent 候选发布。

在「Agent 管理 → 平台自身更新」点击「更新平台并重启」。需同源 HTTPS、已登记的 SSH 主机密钥、管理员 SSH/sudo 验证，以及显式确认数据库备份和离线风险。首次通过管理员验证后，密码或 SSH 私钥、口令和独立 sudo 密码加密存入平台保险库；后续可复用，无须反复输入，亦可在对话框清除。无候选或已是候选版本时按钮禁用。不从公网自动下载、不从浏览器指定路径/命令，也不在生产进程里执行构建或 `git pull`。

独立系统 transient unit 延迟启动固定安装器，以平台普通用户安装，仅重启 `easymanage-server.service`，不改服务单元、证书、配置或 Agent。旧哈希资源保留，先复制资源再切换 HTML，备份所有被替换文件；TLS 使用原 CA 验证本地就绪接口，确认数据库/API 就绪后报告成功。失败则回退文件并再检查就绪，数据库迁移由新 Server 正常启动执行，**已执行迁移不会随文件回退撤销**。生产操作前需管理员准备数据库备份，不能承诺任意 schema 变更的完全回滚。

页面等待结果，连接中断不等于失败且不会重复提交；完成后点击「载入新版平台」。结果保存在 `platform-update-result.json`，独立任务日志见系统 journal 的 `easymanage-platform-update-<UID>.service`。超时或失败需先检查结果和日志，不重复提交。备份及旧候选只保留、不自动清理。第一次启用此入口仍需手动部署支持该功能的平台版本。
