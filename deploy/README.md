# 裸机部署

当前仅支持 Linux 裸机。`windows/` 和 `macos/` 是带接入说明的准备目录，尚无可执行安装器；`docker/` 按要求保留但不支持容器。

## 模式与目录

- `linux/system/`：系统级 systemd 单元与 Server / Full Agent 安装器。
- `linux/user/`：普通用户 Server、Basic Agent、用户单元与静态资源部署。用户模式是有效部署方式，不再称 legacy。
- `linux/bootstrap/`：系统依赖、共享工具链和显式新建 PostgreSQL。
- `linux/operations/`：认证后的提权、更新、重启、账户及证书维护。
- `linux/migrations/`：显式切换部署模式的交接脚本；不是数据库 SQL 迁移。
- `orchestration/`：可信清单、候选发布与分发编排；当前只实现 Linux 目标。
- `examples/`：可发布的配置示例，不含真实节点身份或凭据。
- `database/`：PostgreSQL 通用部署说明。
- `windows/`、`macos/`：原生安装与权限适配的接入清单，不表示已支持。
- `local/`：Git 忽略的本机清单和私有配置，不随项目发布。

系统级配置位于 `/etc/easymanage/`，二进制在 `/usr/local/sbin/`，Server 静态文件在 `/opt/easymanage/web/`，Agent SQLite 在 `/var/lib/easymanage-agent/`。普通用户的资产位于 `~/.local/share/easymanage/`，单元位于 `~/.config/systemd/user/`。目录整理不意味着迁移运行中的服务。Server 的运行模式和 Agent 的权限独立：用户 Server 可以管理 Full Agent。

`orchestration/fleet.py` 是可信 SSH 编排入口，构建脚本和 Agent 管理页面已同步引用。它能发布/暂存 Full 候选版本，也支持用户部署；不是仅支持 Basic 的部署器。已启用的 Full Agent 不通过普通用户更新路径降权，离线/失败的 Full 服务仍需管理员认证处理。新版 Server 优先新入口，同时兼容旧已发布包中的 `deploy/linux/fleet.py`。新发布写入新布局，但不自动删除已发布包的历史文件；本次整理不修改正在运行的包。

## 发布清单

`examples/fleet.example.json` 仅含示例地址和 ID，并标记 `example: true`，部署器拒绝执行。复制为 `deploy/local/fleet.json`，替换主机、端口、SSH 别名、用户、平台 URL 和节点 ID，完成配置后删除 example 标记。已有节点保留真实 ID 和 SQLite 状态，不重新生成身份。默认只读取私有清单；自定义路径用 `--inventory`。目标可显式标记 `platform: "linux"`；旧清单缺失此字段仍按 Linux 处理，其他平台目前拒绝执行。

SSH 主机密钥须预先核验。编排使用普通 SSH 用户和严格密钥检查，不自动运行远端 sudo。Full 实际更新仍需管理员 SSH / sudo 验证。构建及候选发布不更新运行中的服务。

## 系统级部署

先以普通用户构建 `./scripts/run-server.sh --build`。数据库可用本地/远程 PostgreSQL，见 [数据库配置](database/README.md)；不强制安装新数据库或使用特定端口。配置、真实 TLS 和注册密钥放仓库外。

安装器见 [系统模式](linux/system/README.md)，环境准备见 [bootstrap](linux/bootstrap/README.md)。安装会重启 EasyManage，不应在日常更新时重复执行数据库初始化、账户迁移或依赖安装。既有生产平台的运行方式不在目录整理中切换。

现有 `--controller`、`--prepare-fleet`、`--fleet`、`--start-all` 入口操作用户服务，见 [用户模式](linux/user/README.md)。不能因为 Full 暂时离线自动迁移。Windows/macOS 接入要求分别见 [Windows](windows/README.md) 和 [macOS](macos/README.md)。

网页只读监控无平台登录；在线终端、提权、账户变更及强制重启仍验证目标 SSH / sudo 身份。生产部署会真实修改服务；运行时、故障、提权与压力测试只在隔离环境进行。不提交凭据、私钥、运行数据库、备份或 staging 包，不改动其他业务的服务、账户、网络或防火墙。
