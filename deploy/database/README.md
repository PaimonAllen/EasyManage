# PostgreSQL 通用部署

平台数据库使用 PostgreSQL；Agent 的本地身份、硬件缓存与离线队列使用 SQLite。不需要在 Agent 主机部署 PostgreSQL，也不要求平台数据库位于同一机器。

复制 `server/config/server.env.example` 到仓库外，权限设为 `0600`。可配置 `PGHOST`、`PGPORT`、`PGDATABASE`、`PGUSER`、`PGPASSWORD`、`PGSSLMODE` 和 `PGSSLROOTCERT`。也可使用完整 `DATABASE_URL`，它优先于 PG* 配置，原生产 URL 保持不变。PG* 配置自动编码密码中的特殊字符，URL 方式则需要自行进行 URL 编码。支持 IPv6 和 Unix socket 主机参数。

连接池通过 `EASYMANAGE_DATABASE_MAX_CONNECTIONS` 控制（1–64，默认 10）。远程数据库建议 `PGSSLMODE=verify-full` 并配置可信数据库 CA；本地可用 `prefer`，不要为了部署开放整个网段或启用 `trust` 认证。

## 使用已有或远程数据库

由数据库管理员创建独立的非超级用户角色和空数据库，授予此角色该数据库及 schema 的建表、索引和迁移权限。不要复用其他应用的数据库或超级用户账户。连接参数由实际部署环境提供；不固定 PostgreSQL 安装目录、端口或系统登录用户名。

Server 在数据库可用后自动应用 `migrations/` 中的 SQL 迁移，旧迁移不得修改或重排。数据库不可用时，HTTPS 静态页和受管理员验证的恢复接口仍可访问，监控 API 返回 503；不会自动创建数据库或修改数据库服务。

## 新建本机 Ubuntu PostgreSQL

仅在需要安装数据库的新主机，由管理员检查后运行：

```bash
EASYMANAGE_POSTGRES_MAJOR=18 \
EASYMANAGE_DATABASE_NAME=easymanage EASYMANAGE_DATABASE_ROLE=easymanage \
  bash deploy/linux/bootstrap/install_postgresql_and_initialize.sh --apply
```

这个脚本会修改 Apt 仓库、安装包、数据库角色权限和 `/etc/easymanage/server.env`，不是日常更新步骤。版本可配置；端口默认采用已选集群的实际端口，也可显式设 `EASYMANAGE_POSTGRES_PORT`。不为了匹配端口重配置已有集群。脚本生成自己的随机密码，不能代替任意现有数据库的管理流程。已有生产数据库应直接提供连接配置，不运行初始化脚本。`--rotate-password` 是显式密码轮换，需同步所有使用该角色的配置。

当前完整隔离回归使用 PostgreSQL 18；其他 PostgreSQL 版本不宣称已通过回归。此项目没有实现 MySQL / SQLite 平台存储后端。
