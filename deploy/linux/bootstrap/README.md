# Linux 环境准备

这里放首次安装的依赖与数据库准备，不是日常更新入口。

- `install_system_build_dependencies_ubuntu.sh`：安装 Ubuntu 系统构建依赖，会修改 Apt 包状态。
- `install_global_rust_toolchain_ubuntu.sh`：安装共享 Rust 工具链，保留现有用户工具链。
- `install_postgresql_and_initialize.sh --apply`：明确需要新本机数据库时初始化 PostgreSQL；版本、端口、角色和数据库名可配置，见 [数据库说明](../../database/README.md)。已有/远程数据库直接提供配置，不运行初始化脚本。

这些脚本需要管理员权限，可能修改软件包、系统配置或数据库。执行前核验目标及维护窗口；目录整理不执行它们。服务安装器在 [system](../system/README.md)。
