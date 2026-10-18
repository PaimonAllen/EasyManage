# 隔离集成测试

SSH 长连接回归复用 `--machine-onboarding`：目标上报地址实际不可达时自动使用真实反向 HTTPS 通道，要求收到真实 Agent 心跳；检查稳定两条连接、指纹/健康查询与重启不新增握手、连接中断后的退避恢复、三连接硬上限、禁用转发时明确失败、显式断开/重连及软删除恢复后身份不变。端口转发由隔离 Paramiko fixture 提供，不修改生产 sshd，也不代表生产管理员已允许转发。`test_ssh_network_config.py` 检查配置切换的原 UUID/额外设置保留、私有备份、CA/身份/URL/注入/符号链接拒绝与秘密不回显。Rust 单测补充握手预算与有界重连退避；前端 SSR 检查自动/直连/SSH 三种选项、最多三连接说明及 TLS 校验提示。

`full_ssh_update_smoke.py` 同样传入宿主 net/mnt ID，在嵌套私有 root namespace 中检查固定 Full 更新器的正常替换、错误摘要/UUID 拒绝、服务重启失败回退，以及从未有 Basic 配置的 Full 节点切换网络。所有情况保留原 SQLite 字节；使用 mock systemd，不接触生产 root 服务。

生产主机不直接运行测试。`isolated_smoke.py` 必须在独立网络和挂载命名空间、私有 `/tmp`、非 root 用户下执行；它会拒绝原主机命名空间。该环境不是完整虚拟机。

先在工作区编译，测试程序不在此步骤运行：

```bash
./scripts/run-server.sh --build
cargo test --workspace --no-run --jobs 2
```

已有 `bwrap` 和 PostgreSQL 18 二进制时，可使用下面的隔离测试入口。它遮蔽主机 home、运行时 socket、设备和进程，仅以只读方式映射工作区；网络只有隔离环境自己的 loopback：

```bash
bwrap --unshare-all --die-with-parent --new-session \
  --ro-bind / / --dev /dev --proc /proc \
  --tmpfs /tmp --tmpfs /run --tmpfs /home --tmpfs /sys \
  --dir /tmp/work --ro-bind "$PWD" /tmp/work --chdir /tmp/work \
  /usr/bin/python3 /tmp/work/tests/integration/isolated_smoke.py \
  --server-binary /tmp/work/target/release/easymanage-server \
  --agent-binary /tmp/work/target/release/easymanage-agent \
  --host-netns "$(readlink /proc/self/ns/net)" \
  --host-mntns "$(readlink /proc/self/ns/mnt)"
```

测试自建临时 PostgreSQL、TLS 和节点数据，结束后停止子进程并移除其临时数据。不会连接生产数据库、SSH、Nginx 或 systemd。覆盖无登录只读 API、Agent 机器认证、快照排序、24 小时进程历史聚合、只读块设备树/未挂载分区的协议兼容、目标信息脱敏和跨源运维拒绝；不代表真实浏览器、生产 Full 提权及服务交接已通过验证。

Rust 单元测试同样在上述隔离环境执行编译得到的 `target/debug/deps/` 测试二进制，不在生产环境直接执行 `cargo test`。

可选 `--machine-onboarding` 回归需临时测试 venv 的 Paramiko 3.5.1（不是平台依赖）和 C 编译器。仍须使用上面的私有网络、`/home` 与 `/tmp`，将 venv 只读映射至 `/tmp/ssh-env` 并用其 Python 调用隔离入口。测试会生成临时 SSH 密钥和凭据，SSH/SCP、摘要校验、用户安装脚本与 Agent 上报是真实执行；systemd 用户 manager 为 mock。`fixtures/ssh_loopback_redirect.c` 仅预加载到该临时 Server，将两个合成内网 IP 的 SSH 连接转到隔离 loopback，不改网络配置、不接触生产 SSH。严禁对生产进程使用该测试 shim。

新增机器鉴权回归包含旧版密码请求兼容、未加密/加密 PEM RSA 私钥、带口令 OpenSSH Ed25519 私钥完成真实部署、错误口令和指纹阻断；还检查解析错误不回显秘密，任务日志、运行日志、清单和安装后的 Agent 文件不含密码/私钥/口令。所有密钥均在隔离测试运行时生成，不使用个人 SSH 密钥。`web/tests/machineAuthentication.test.ts` 检查请求序列化后及异常时的引用清理，SSR 检查鉴权选项、密码/口令掩码和不回显正文的私钥上传。

普通用户 SSH 回归使用 mock `Linger=no` 和私有 cron 表，运行实际兼容守护与 Agent，验证 SSH 全部断开后仍继续上报、崩溃重启、原身份保持及自动恢复能力提示；不会读取/写入宿主 crontab。`test_agent_user_control.py` 检查 cron 标记块保留、不可读表不覆盖、缺失 cron 的降级、禁用标记和 PID 复用保护。`compatibility_agent_smoke.py` 在相同隔离入口内接收宿主 net/mnt ID，使用合成 Agent 检查双重单例、守护进程被杀后的孤儿防重、停止/重启及有界日志。均不等同真实 logind 清理策略、cron 调度或生产 systemd 的验证。

`compatibility_promotion_smoke.py` 同样接收宿主 net/mnt ID，在嵌套私有 root 文件系统中使用 mock systemd/runuser 检查兼容模式提权顺序与 Full 启动失败恢复：必须以原用户执行停止，停止后才能安装 Full，保留原身份字节，失败时尝试恢复此前启用的兼容实例。没有执行生产 sudo、切换真实用户或操作真实服务。

`first_full_install_smoke.py` 同样必须在上述隔离环境运行并传入宿主 net/mnt namespace ID。它在嵌套 root 用户 namespace 内构造私有系统目录，只使用 mock systemd；覆盖普通 SSH / root 暂存布局的首次 Full、配置路径重写、root 所有权、错误摘要、已有单元/身份拒绝、符号链接/不安全系统目录拒绝、启动失败撤回与恢复数据保留。真实 SSH 回归还核对 Basic 暂存包仅在 SSH 用户 home 内、sudo 拒绝时不上传、不回退为 Basic，并执行实际 Full 安装器与 namespace-root Agent，确认真实 Full / UID 0 心跳。sudo 鉴权及 systemd 均为 mock，不是生产 root/sudo 或真实 systemd 的端到端验证。

部署纯逻辑回归为 `test_fleet.py`；应急重启 mock 回归为 `force_restart_smoke.py`。后者同样在上述隔离环境调用，并传入两个位置参数：宿主 net、mnt 命名空间 ID。它在嵌套私有命名空间内模拟 systemd/runuser，不联系生产服务。真实 Agent 回归同时检查少量线程、实时上报、慢速采样缓存和重启后身份保持。

温度回归使用合成 hwmon 数据，覆盖 CPU / SSD 分类、机械盘排除、错误读数、缓存、采集时间与数据库历史兼容。隔离入口遮蔽 `/sys`，真实 Agent 测试不会读取生产硬件传感器。前端 `web/tests/temperature.test.ts` 检查缺失读数与采样去重；`web/tests/charts-smoke.mjs` 使用 Vite + React 服务端渲染检查五图布局、三条独立温度曲线及旧版数据，要求传入宿主网络 / 挂载命名空间 ID。两者也必须在上述隔离环境运行；服务端渲染检查不等同真实浏览器截图测试。

平台自身更新回归：`test_platform_release.py` 为临时目录纯逻辑测试，覆盖发布不影响运行服务、逐文件摘要检查、允许组可写的 home 上级目录、拒绝平台自身目录组写入及越界/符号链接、成功安装只改变平台、就绪失败回退和版本比较。机器管理 Rust 单测同样覆盖共享 home 下私有清单目录可用，清单目录组写入和符号链接仍拒绝。`platform_update_smoke.py` 在上述隔离环境运行并接收宿主 net/mnt 命名空间 ID，通过嵌套私有 namespace 模拟 systemd/runuser，覆盖管理员固定调度、错误单元/候选拦截、重复任务拒绝和 Agent 不被触及。HTTP smoke 覆盖平台更新接口跨源拒绝与无候选禁用。所有测试均不重启生产服务；纯逻辑测试也不会真正联系 systemd 或 HTTPS。
