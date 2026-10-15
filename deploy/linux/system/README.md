# Linux 系统级安装

这里放系统级服务单元和安装器。普通用户模式位于 [user](../user/README.md)，依赖和数据库初始化位于 [bootstrap](../bootstrap/README.md)，日常管理操作位于 [operations](../operations/README.md)。

- `install_easymanage_server_and_agent.sh`：安装已构建的系统级 Server / Full Agent，保留数据库连接和自定义设置；需要真实 `/etc/easymanage/server.env`，会重启 EasyManage。
- `install_easymanage_agent.sh BINARY CA ENV UNIT`：安装暂存 Agent 系统服务并重启。
- `install_easymanage_all.sh [--remote-manual]`：双主机编排，必须显式设置 `EASYMANAGE_REMOTE_TARGET`、`EASYMANAGE_SERVER_ADDRESS`，无生产地址默认值；manual 只暂存远端管理员包。
- `install_staged_easymanage_agent.sh`：管理员包摘要校验和安装，只允许受限 staging 路径。

执行前检查实际目标、服务模式和磁盘空间；不把安装、密码轮换或迁移当成日常更新。这轮目录整理没有执行这些脚本，没有重启服务、安装数据库或改变生产权限。
