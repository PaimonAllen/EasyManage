# 构建与运行入口

`run-server.sh` 和 `run-agent.sh` 无参数时以前台当前用户运行已有 release 二进制，不自动构建或 sudo。不要在已有 Agent 服务运行时额外启动相同身份的前台实例。

- `--build`：低优先级双任务构建 Rust、Web、VitePress；有私有清单和可信注册表时发布候选，不更改运行服务。
- `--publish-agent`：发布当前候选，不重新构建或重启。
- `--publish-platform`：只发布已构建的平台、前端与手册候选；可随后在「Agent 管理 → 平台自身更新」管理员确认安装，不从网页执行构建或 git 拉取。
- `--controller`：更新本机用户平台并重启，保留 Agent。
- `--controller --preserve-controller-unit`：保留当前平台 service 配置和启用状态，仅替换产物并重启。
- `--prepare-fleet`：核验 SSH、准备私有安装包和可信清单，不切换服务。
- `--fleet`：更新用户平台与 Basic Agent；保留已有 Full 的管理员边界。
- `--start-all`、`--status`：启动/查询已有用户服务；已启用 Full 不启动重复 Basic，停止的 Full 要管理员处理。

后五项保持普通用户部署兼容，用户模式并未废弃。编排入口在 `deploy/orchestration/fleet.py`，普通用户服务资产在 [user](../deploy/linux/user/README.md)，系统服务资产在 [system](../deploy/linux/system/README.md)。

默认清单为 Git 忽略的 `deploy/local/fleet.json`，绝不默认执行公开示例：

```bash
./scripts/run-server.sh --status --inventory deploy/local/fleet.json
```

自定义 SSH 配置可通过 `EASYMANAGE_REMOTE_SSH_CONFIG` 指定。集群编排拒绝 root；使用经过核验的主机密钥和实际 SSH 账户，保留既有节点 ID。

前台入口兼容当前用户的 `~/.easymanage/config/*.env`，可显式设置 `EASYMANAGE_ENV_FILE`。系统配置和数据库模板见 [数据库说明](../deploy/database/README.md)。`diagnostics/` 放只读磁盘与 Nginx 检查。实际部署会影响生产服务，不属于测试。

发布前运行 `python3 scripts/check-repository.py --staged`，核对索引包含源码、锁文件及迁移，没有私有/生成文件。检查仅输出路径和行号，不显示发现的秘密值；属于定向检查，不代替完整秘密扫描。
