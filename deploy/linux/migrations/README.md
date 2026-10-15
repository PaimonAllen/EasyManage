# Linux 部署模式交接

这里放明确请求的部署布局迁移，以及系统级 → 用户模式交接：`handover.sh`、`migrate-agent-to-user.sh`、`migrate_platform_to_user.sh`。平台交接脚本从源码运行；Agent 交接脚本通常在摘要已验证、文件名保持不变的私有暂存包中运行。

不是数据库迁移：SQL 迁移仍在仓库根 `migrations/`。正常更新不得调用这些交接脚本。交接会停止/切换服务、迁移配置和 SQLite 身份，须管理员确认目标、权限、备份与维护窗口；无完整身份时拒绝执行。恢复数据可能包含秘密，不批量删除。

用户部署说明见 [user](../user/README.md)。目录整理不执行迁移，也不改变现有 Full / Basic 权限。

## 统一用户目录到 ~/.easymanage

`migrate-user-root.py` 从旧的 `~/.local/share/easymanage` 原地重命名到 `~/.easymanage`，保留所有 SQLite 文件、UUID、配置、CA、候选和历史恢复数据；只重启迁移前已运行的用户服务，绝不把 Full 降回 Basic。Full 的 `/etc/easymanage`、`/var/lib/easymanage-agent` 和 root 系统服务不移动、不调用 sudo。systemd 用户单元仍在标准目录。已有新目录时拒绝合并；不创建旧路径软链接。

先以现有部署用户预演：`python3 deploy/linux/migrations/migrate-user-root.py --server-binary /准确路径/新构建的/easymanage-server`。确认维护窗口后添加 `--apply`。只有运行平台的主机需要传新二进制；仅有 Agent 用户目录的远端省略该参数。迁移前备份配置、单元与旧平台二进制到 `~/.easymanage-migration.<随机值>`，平台就绪失败时尝试还原。旧平台候选索引暂存到恢复目录，避免安装回旧路径版本；迁移完成后重新发布新候选。脚本不修改数据库或根服务，不清理恢复文件。
