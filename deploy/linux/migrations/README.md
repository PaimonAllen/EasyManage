# Linux 部署模式交接

这里只放明确请求的系统级 → 用户模式交接：`handover.sh`、`migrate-agent-to-user.sh`、`migrate_platform_to_user.sh`。平台交接脚本从源码运行；Agent 交接脚本通常在摘要已验证、文件名保持不变的私有暂存包中运行。

不是数据库迁移：SQL 迁移仍在仓库根 `migrations/`。正常更新不得调用这些交接脚本。交接会停止/切换服务、迁移配置和 SQLite 身份，须管理员确认目标、权限、备份与维护窗口；无完整身份时拒绝执行。恢复数据可能包含秘密，不批量删除。

用户部署说明见 [user](../user/README.md)。目录整理不执行迁移，也不改变现有 Full / Basic 权限。
