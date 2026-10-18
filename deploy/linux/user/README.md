# Linux 普通用户模式

这里放普通用户 Server / Basic Agent 和 systemd 用户服务，系统级单元与安装器位于 [system](../system/README.md)。用户模式并未废弃；用户 Server 可以管理 Full Agent，不自动切换生产模式或把已提权 Agent 降回 Basic。

程序、配置、TLS 和 SQLite 状态在 `~/.easymanage/`。用户不可见的指标保持未知。Basic 自动优先选择已有 manager + linger 的 systemd 用户服务（单元在 `~/.config/systemd/user/`），否则使用 `agent-user-control.py` 普通用户后台守护，不要求先启用 linger，也不调用 sudo。兼容模式日志有界，允许用户 cron 时添加独立标记块作开机/每分钟恢复；cron 不可用时仍允许部署，但必须明确提示恢复能力受限。平台服务本身仍使用 systemd 用户服务，不在本次兼容范围内。

`scripts/run-server.sh` 的 `--controller`、`--prepare-fleet`、`--fleet`、`--start-all` 保持兼容。默认读取私有 `deploy/local/fleet.json`；Full 的更新、重启或强制重启走管理员验证路径。

## 显式迁入用户模式

交接脚本位于 [migrations](../migrations/README.md)。只有明确需要切换到用户模式时，普通用户先 `--build`、`--prepare-fleet`；管理员核对准确 staging 路径。平台主机 root 终端可执行：

```bash
EASYMANAGE_OPERATOR_USER=operator bash deploy/linux/migrations/migrate_platform_to_user.sh
```

替换 operator 为实际服务用户。远端管理员执行 prepare 输出的精确 `handover.sh` 命令，不用通配符寻找 staging。交接核验摘要、保留 SQLite 身份和恢复数据，切换服务并启用 linger，会短暂中断监控。缺少原配置或身份时拒绝迁移。恢复文件可能含秘密，按精确路径管理，不批量清理状态或 `/tmp`。

## 用户平台维护

只允许目标 SSH 端口时，可在 GUI 使用 SSH 长连接：平台通常维持控制和回环 HTTPS 上报两条连接，含附加管理员身份最多三条；不依赖目标回连平台。需目标允许 SSH 远程回环转发，平台证书包含 `127.0.0.1`。凭据仅驻留平台内存，重启后重新鉴权；不会自动修改 sshd 或降低 Full 权限。见 [部署说明](../../../docs/deployment.md#仅开放-ssh-端口的机器)。

`deploy_web_static.sh` 只更新已有用户平台静态资源，保留旧资源和回滚文件，不重启服务；默认 URL `https://127.0.0.1:8443`，可设 `EASYMANAGE_PLATFORM_URL`。目标目录必须与当前用户的配置匹配。

`../operations/renew_controller_tls_for_interfaces.sh --apply` 由 root 显式执行，要求 `EASYMANAGE_OPERATOR_USER`；复用 CA / 私钥，按本机物理及虚拟网卡地址重签叶证书并短暂重启用户平台。失败尝试恢复；不是日常更新步骤。页面与 API 必须同 HTTPS 主机和端口，签入证书不代表浏览器已信任 CA。
