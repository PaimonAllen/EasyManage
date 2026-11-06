# PostgreSQL 通用部署

平台数据库使用 PostgreSQL；Agent 的本地身份、硬件缓存与离线队列使用 SQLite。不需要在 Agent 主机部署 PostgreSQL，也不要求平台数据库位于同一机器。

复制 `server/config/server.env.example` 到仓库外，权限设为 `0600`。可配置 `PGHOST`、`PGPORT`、`PGDATABASE`、`PGUSER`、`PGPASSWORD`、`PGSSLMODE` 和 `PGSSLROOTCERT`。也可使用完整 `DATABASE_URL`，它优先于 PG* 配置，原生产 URL 保持不变。PG* 配置自动编码密码中的特殊字符，URL 方式则需要自行进行 URL 编码。支持 IPv6 和 Unix socket 主机参数。

连接池通过 `EASYMANAGE_DATABASE_MAX_CONNECTIONS` 控制（1–64，默认 4）。远程数据库建议 `PGSSLMODE=verify-full` 并配置可信数据库 CA；本地可用 `prefer`，不要为了部署开放整个网段或启用 `trust` 认证。

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

小规模部署的数据库连接池默认最多 4 条，空闲连接 60 秒后释放。已有显式连接池配置继续生效。采集频率、历史保留时间和 PostgreSQL 持久化保证不变。

可选低内存配置见 `postgresql-low-footprint.conf.example`：将专用 PostgreSQL 的 `shared_buffers` 设为 32 MiB，替代常见的 128 MiB 默认值。它只减少共享缓冲池预算，不限制整个 PostgreSQL 服务或操作系统文件缓存。不要应用到 Gitea 或其他业务共用的实例；大量节点、历史查询或磁盘扫描可能需要更大的缓冲池。

例如当前专用 PostgreSQL 18/main 使用 5433，而 Gitea 使用独立的 14/main。管理员确认实例后可执行以下步骤；**重启会短暂中断平台数据库连接**，Agent 会保留队列并重试。先确认 `conf.d/80-easymanage-low-footprint.conf` 不存在，已有文件应先备份，禁止直接覆盖。此配置不会自动部署。

```bash
sudo install -m 0644 deploy/database/postgresql-low-footprint.conf.example /etc/postgresql/18/main/conf.d/80-easymanage-low-footprint.conf
sudo systemctl restart postgresql@18-main.service
```

回退时移走这个新建的配置文件并再次重启同一集群。不要对生产历史表执行 `VACUUM FULL` 或直接删除数据来追求短期占用数字。

Schema 22 将低频清单保存在 `node_snapshot_inventory`，实时快照只更新较小的指标 JSON；`metrics_snapshot` 提供合并后的只读视图。升级保留原节点、历史、人工复核、权限和采样时间。新旧 Agent 均可向新平台上报；协议 3 仅在服务端确认缓存后启用，缓存丢失时完整重传，旧平台会拒绝缓存引用并触发兼容重传。紧急回退先停平台，执行 `rollback-inventory-cache.sql`，再恢复旧程序；它会合并回原字段，不删除历史。

新 Agent 的 SQLite 队列共享清单内容，但保持每份历史快照的版本和持久确认；旧格式队列仍可读取。回退 Agent 前应先让新版本排空队列，或恢复安装器保留的一致备份；不能让旧程序直接读取尚含新版引用的队列。
