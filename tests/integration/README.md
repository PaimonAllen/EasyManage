# 隔离集成测试

HTTP/2 浏览器授权：`isolated_smoke.py --machine-admin-http2 --agent-binary ...` 使用下述 Paramiko 环境和支持 HTTP/2 的 `/usr/bin/curl`。在私有 TLS/SSH/PostgreSQL 中完成首次管理员验证，用 stdin 提供合成 Cookie，验证分段 Cookie 排在其他 Cookie 后仍可读取管理员授权、无 Cookie 不授权、保存凭据续期兼容 HTTP/1.1，以及平台重启后的授权保留；不调用生产运维接口。来源校验单测覆盖物理 LAN、任意私网 VLAN、ZeroTier、Tailscale、IPv6 overlay 和内部 DNS，不以客户端网段作为授权。

完全停止 Agent：`agent_service_stop_smoke.py` 使用私有嵌套命名空间和 mock systemd，验证 Full、systemd Basic 和真实兼容守护的停服、禁用自动拉起、错误 UUID / Full 账户拒绝，以及文件 / 身份库字节保留。`isolated_smoke.py --machine-agent-stop --agent-binary ...` 另需下述 Paramiko venv；执行真实 SSH、真实 Agent 和临时 PostgreSQL，区分暂停后原进程轻量心跳与停止后进程退出，验证三连接上限、保存凭据授权、停止后禁止直接恢复 / 重启 / SSH 重连、原 UUID 重新部署恢复和平台重启不自动拨号。Full 系统服务管理仍为隔离 mock，不在生产上执行停服验证。

存储挂载回归：Agent `platform::linux::mounts::tests` 使用合成 mountinfo 验证 NFS/autofs 清单、绑定目录身份、未知容量、转义路径和凭据不输出，不连接网络文件系统。HTTPS smoke 验证新增元数据入库与旧 Agent 兼容，且未知 NFS I/O 不影响已有块设备的速率。前端 `storageMounts.test.ts` 和 SSR 验证启动分区默认隐藏、真实布局保留、共享容量和未知值显示。

全机磁盘扫描回归在私有 mount namespace 创建两个小型 tmpfs 和重叠的 bind mount，验证各文件系统覆盖、绑定去重、真实路径、全局预算、取消和不可读主入口的绑定回退；生产默认仍禁止扫描 tmpfs。HTTP 用 `all_local` 加 1 条预算验证新范围持久化，在发现或枚举文件系统前停止，不扫描宿主目录。

磁盘扫描无限制回归：合成 32,010 个文件，并模拟超过原时间上限，验证文件计入汇总、只保存一个目录且不生成临时 SQLite。HTTP/PostgreSQL 用 26,001 个子目录验证完整目录排序、跨页搜索、末页和祖先导航；root 结果沿用平台普通读取权限，启动或取消 root 扫描仍要求管理员授权。测试保留有限预算、手动取消和旧版快照的回归。

磁盘空间分析：`disk_space_scanner_smoke.py` 在上述隔离环境中接收宿主 net/mnt ID，使用合成目录验证实际/逻辑大小、稀疏文件、硬链接、符号链接竞态、FIFO、无权限目录、取消和条目/深度限制。嵌套私有 root/mount namespace 验证同设备 bind mount 不被跨越。`isolated_smoke.py --disk-space` 运行真实临时 HTTPS/PostgreSQL 和固定扫描器，只读取只读绑定的仓库测试夹具，验证权限、输入、跨源拦截、持久任务和虚拟文件系统拒绝。`web/tests/diskSpace.test.ts` 检查父级分母和矩形面积，SSR 检查不完整值提示及文件名转义。测试不连接生产 SSH、不扫描生产数据目录。

管理员凭据回归可用 `--machine-admin-credentials` 快速入口（自动启用 SSH fixture，仍须下述私有网络、目录遮蔽及 Paramiko venv）。它执行真实 Full 安装和 SSH 密码/加密私钥认证，root 权限检查和服务管理为 mock；验证错误口令/sudo 不保存或覆盖、普通 SSH 授权不能用于管理员操作、保存后不重传秘密即可重启、授权续期、平台重启持久化以及清除管理员密文不影响普通连接凭据。完整 `--machine-redeployment` 也执行这些检查。回归遵守原操作频率和 SSH 恢复互斥，不降低生产保护来通过测试。

网络资源统计回归：Rust 单测检查计数/取消/连接生命周期、饱和运算及流式 Body 保留大小提示；HTTP smoke 比较准确消费的请求正文长度、响应正文、失败计数与内容不留存，真实 Agent 检查客户端分类、生命周期重置及平台读取的认证快照。SSH fixture 同时核对真实握手计数、exec/SCP/反向隧道字节和原有最多三连接限制。前端 SSR 与 `networkActivity.test.ts` 覆盖未知/离线、首次/重复/重启/回退时不伪造速率及跨层不可相加提示。全部在本文隔离环境运行，不抓取生产流量。

权限检查开关回归：验证独立的节点开关、旧 Agent / 只支持用户开关的 Agent 的策略兼容、拒绝跨源和过期设置、停采时人工标记休眠而非删除及恢复后变更证据使标记失效。真实 Agent 测试检查暂停权限审计而保留其他指标、双开关跨重启持久化、单独恢复用户不会恢复权限检查、恢复权限后生成新审计；平台/数据库重启后仍保留设置。前端检查每台机器均有可访问开关，所有测试仅在隔离环境执行。

用户状态开关回归：默认 HTTP smoke 检查每节点持久设置、跨源/非法布尔值/未知节点/过期页面拒绝及旧版策略兼容；配合 `--agent-binary` 验证真实 Agent 停止用户上报但保留其他指标和权限检查，Agent 重启首次采样仍保持关闭，再开启时重新扫描。平台和 PostgreSQL 恢复后开关仍保留。Rust 单测覆盖无会话/组/用户进程计数、取消旧扫描、SQLite 持久化；前端 SSR 区分确认、待应用、离线和旧版需更新状态。全部在下述隔离环境执行，不操作生产机器。

SSH 长连接回归复用 `--machine-onboarding`：目标上报地址实际不可达时自动使用真实反向 HTTPS 通道，要求收到真实 Agent 心跳；检查稳定两条连接、指纹/健康查询与重启不新增握手、连接中断后的退避恢复、三连接硬上限、禁用转发时明确失败、显式断开/重连及软删除恢复后身份不变。端口转发由隔离 Paramiko fixture 提供，不修改生产 sshd，也不代表生产管理员已允许转发。`test_ssh_network_config.py` 检查配置切换的原 UUID/额外设置保留、私有备份、CA/身份/URL/注入/符号链接拒绝与秘密不回显。Rust 单测补充握手预算与有界重连退避；前端 SSR 检查自动/直连/SSH 三种选项、最多三连接说明及 TLS 校验提示。

凭据保险库回归包含密码/私钥/口令的认证加密往返、随机 nonce、篡改/跨节点替换/错误主密钥/缺失密钥/权限/符号链接拒绝，以及节点作用域浏览器 Cookie、过期凭证、暂停恢复和删除密文。真实 SSH fixture 检查无 Cookie 不能使用保存的凭据、同浏览器免输入重连、实际平台重启后自动恢复并收到新心跳；随后使用真实时间等待三次 60 秒重试，确认停止并释放所有连接，再通过保存的密文手动恢复。该组测试会比旧版多花数分钟，不缩短生产重试间隔。所有 SSH 密钥和主密钥均为隔离临时生成，不使用生产凭据。

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
  --tmpfs /var --tmpfs /etc/easymanage --tmpfs /etc/systemd/system --tmpfs /usr/local/sbin \
  --dir /tmp/work --ro-bind "$PWD" /tmp/work --chdir /tmp/work \
  /usr/bin/python3 /tmp/work/tests/integration/isolated_smoke.py \
  --server-binary /tmp/work/target/release/easymanage-server \
  --agent-binary /tmp/work/target/release/easymanage-agent \
  --host-netns "$(readlink /proc/self/ns/net)" \
  --host-mntns "$(readlink /proc/self/ns/mnt)"
```

测试自建临时 PostgreSQL、TLS 和节点数据，结束后停止子进程并移除其临时数据。不会连接生产数据库、SSH、Nginx 或 systemd。覆盖无登录只读 API、Agent 机器认证、快照排序、24 小时进程历史聚合、只读块设备树/未挂载分区的协议兼容、目标信息脱敏和跨源运维拒绝；不代表真实浏览器、生产 Full 提权及服务交接已通过验证。

Rust 单元测试同样在上述隔离环境执行编译得到的 `target/debug/deps/` 测试二进制，不在生产环境直接执行 `cargo test`。

代理开关回归验证每节点独立持久化、同源/布尔值/并发检查、旧报告和轻量控制的能力协商，以及平台重启后的保存状态。真实 Agent 用例在私有网络中启动命名为 `mihomo` 的合成进程，仅监听隔离 loopback，不转发流量；核对关闭后进程仍活着但不再报告代理，重启后保持关闭，重新开启取得新监听快照。Collector 单测覆盖取消旧任务、清空代理缓存且保留其他清单；SSR 检查每机开关、隐藏旧实例、统计排除和旧平台提示。不接触生产代理或外部网络。

整机监控暂停回归覆盖同源、UUID、真实布尔值、并发版本、原 Agent token 鉴权及旧 Agent 待确认；数据库恢复/平台重启保留暂停和确认。真实 Agent 用例验证停止完整/实时指标、队列不增长、暂停中重启仍不启动采集，以及实际等待 60 秒轻量控制和页面恢复后的新样本；不缩短生产控制周期。测试不会暂停生产节点。SSR 覆盖暂停确认/待确认/旧版/恢复等待状态；不等同真实浏览器交互测试。

旧版恢复兼容回归检查：不支持控制协议的 Agent 在用户明确恢复后，真实新鲜报告和实时指标可正常展示，不伪造控制确认；新版 Agent 仅控制确认但没有恢复版本的新样本时仍不显示在线。

可选 `--machine-onboarding` 回归需临时测试 venv 的 Paramiko 3.5.1（不是平台依赖）和 C 编译器。仍须使用上面的私有网络、`/home` 与 `/tmp`，将 venv 只读映射至 `/tmp/ssh-env` 并用其 Python 调用隔离入口。测试会生成临时 SSH 密钥和凭据，SSH/SCP、摘要校验、用户安装脚本与 Agent 上报是真实执行；systemd 用户 manager 为 mock。`fixtures/ssh_loopback_redirect.c` 仅预加载到该临时 Server，将两个合成内网 IP 的 SSH 连接转到隔离 loopback，不改网络配置、不接触生产 SSH。严禁对生产进程使用该测试 shim。

SSH fixture 还强制遮蔽 `/var`、`/etc/easymanage`、`/etc/systemd/system` 和 `/usr/local/sbin`，对私有环境执行真实安装残留检查，不读取生产 Agent 路径。`--machine-redeployment` 自动启用 onboarding，并增加实际卸载归档后原 UUID 的重新部署、在线/跨机器身份/覆盖字段/重复任务拒绝、历史/名称/备注保留、原 SSH 通道复用与旧 Agent token 失效验证；该入口跳过单独的三分钟重试耗尽用例，后者仍由 `--machine-onboarding` 覆盖。Full 级别不降权及请求不可覆盖地址/模式另有 Rust 单测，前端 SSR 检查确认窗口、原 ID/权限展示及已保存凭据模式。

覆盖修复回归额外验证：不同 UUID 在上传前拒绝；同一 UUID 的停机残留允许覆盖，即使原连接凭据已失效也能注册恢复，保留本地历史哨兵与全部已有平台历史，创建一致 SQLite 恢复副本且保持三连接限制；之后再单独测试已卸载后的全新安装/凭据轮换。`first_full_install_smoke.py` 同时检查 root Full 覆盖修复、原身份/队列/关闭采集设置保留、仅重置连接凭据、不同 UUID 拒绝和启动失败后恢复旧文件/凭据；仅使用私有 root namespace 及 mock systemd，不能当作生产 root 服务交接验证。

新增机器鉴权回归包含旧版密码请求兼容、未加密/加密 PEM RSA 私钥、带口令 OpenSSH Ed25519 私钥完成真实部署、错误口令和指纹阻断；还检查解析错误不回显秘密，任务日志、运行日志、清单和安装后的 Agent 文件不含密码/私钥/口令。所有密钥均在隔离测试运行时生成，不使用个人 SSH 密钥。`web/tests/machineAuthentication.test.ts` 检查请求序列化后及异常时的引用清理，SSR 检查鉴权选项、密码/口令掩码和不回显正文的私钥上传。

普通用户 SSH 回归使用 mock `Linger=no` 和私有 cron 表，运行实际兼容守护与 Agent，验证 SSH 全部断开后仍继续上报、崩溃重启、原身份保持及自动恢复能力提示；不会读取/写入宿主 crontab。`test_agent_user_control.py` 检查 cron 标记块保留、不可读表不覆盖、缺失 cron 的降级、禁用标记和 PID 复用保护。`compatibility_agent_smoke.py` 在相同隔离入口内接收宿主 net/mnt ID，使用合成 Agent 检查双重单例、守护进程被杀后的孤儿防重、停止/重启及有界日志。均不等同真实 logind 清理策略、cron 调度或生产 systemd 的验证。

`compatibility_promotion_smoke.py` 同样接收宿主 net/mnt ID，在嵌套私有 root 文件系统中使用 mock systemd/runuser 检查兼容模式提权顺序与 Full 启动失败恢复：必须以原用户执行停止，停止后才能安装 Full，保留原身份字节，失败时尝试恢复此前启用的兼容实例。没有执行生产 sudo、切换真实用户或操作真实服务。

`first_full_install_smoke.py` 同样必须在上述隔离环境运行并传入宿主 net/mnt namespace ID。它在嵌套 root 用户 namespace 内构造私有系统目录，只使用 mock systemd；覆盖普通 SSH / root 暂存布局的首次 Full、配置路径重写、root 所有权、错误摘要、已有单元/身份拒绝、符号链接/不安全系统目录拒绝、启动失败撤回与恢复数据保留。真实 SSH 回归还核对 Basic 暂存包仅在 SSH 用户 home 内、sudo 拒绝时不上传、不回退为 Basic，并执行实际 Full 安装器与 namespace-root Agent，确认真实 Full / UID 0 心跳。sudo 鉴权及 systemd 均为 mock，不是生产 root/sudo 或真实 systemd 的端到端验证。

部署纯逻辑回归为 `test_fleet.py`；应急重启 mock 回归为 `force_restart_smoke.py`。后者同样在上述隔离环境调用，并传入两个位置参数：宿主 net、mnt 命名空间 ID。它在嵌套私有命名空间内模拟 systemd/runuser，不联系生产服务。真实 Agent 回归同时检查少量线程、实时上报、慢速采样缓存和重启后身份保持。

温度回归使用合成 hwmon 数据，覆盖 CPU / SSD 分类、机械盘排除、错误读数、缓存、采集时间与数据库历史兼容。隔离入口遮蔽 `/sys`，真实 Agent 测试不会读取生产硬件传感器。前端 `web/tests/temperature.test.ts` 检查缺失读数与采样去重；`web/tests/charts-smoke.mjs` 使用 Vite + React 服务端渲染检查五图布局、三条独立温度曲线及旧版数据，要求传入宿主网络 / 挂载命名空间 ID。两者也必须在上述隔离环境运行；服务端渲染检查不等同真实浏览器截图测试。

网络盘容量回归：`network_capacity_smoke.py` 在上述私有 namespace 运行，传入宿主 net/mnt ID，只对私有 tmpfs 查询容量；核对跨进程文件锁、受限输出和挂载替换拒绝。Agent `platform::linux::network_capacity::tests` 验证真实子进程超时回收、阻塞任务不会阻塞采样或重复启动、失败保留缓存、重启后的小时级间隔、共享挂载去重和 UID 缓存隔离；同样在隔离环境执行。HTTP 集成测试验证容量采集时间和错误状态往返数据库；SSR 验证缓存值、采集时间、超时与过期提示。未在生产环境制造 NFS 故障。

磁盘空间保护回归：运行隔离入口 `--disk-space` 时，在私有挂载 namespace 中提供 `/tmp/easymanage-low-space` 的 16 MiB tmpfs。新版 bwrap 可用 `--size 16777216 --tmpfs /tmp/easymanage-low-space`；旧版在完整隔离的 bwrap 内以 namespace root 挂载，再通过嵌套 user namespace 映射回非 root UID 执行测试（PostgreSQL 测试拒绝 root）。测试将私有存储检查路径临时指向该空盘，验证 HTTP 507 和没有启动任务；随后在私有 PostgreSQL 用临时触发器模拟 SQLSTATE `53100`，检查准确错误及不泄漏诊断。没有填满任何生产或测试磁盘。Rust 单测核对 2 GiB 边界及正常收尾的不完整结果保留；SSR 检查空间保护提示。

平台自身更新回归：`test_platform_release.py` 为临时目录纯逻辑测试，覆盖发布不影响运行服务、逐文件摘要检查、允许组可写的 home 上级目录、拒绝平台自身目录组写入及越界/符号链接、成功安装只改变平台、就绪失败回退和版本比较。机器管理 Rust 单测同样覆盖共享 home 下私有清单目录可用，清单目录组写入和符号链接仍拒绝。`platform_update_smoke.py` 在上述隔离环境运行并接收宿主 net/mnt 命名空间 ID，通过嵌套私有 namespace 模拟 systemd/runuser，覆盖管理员固定调度、错误单元/候选拦截、重复任务拒绝和 Agent 不被触及。HTTP smoke 覆盖平台更新接口跨源拒绝与无候选禁用。所有测试均不重启生产服务；纯逻辑测试也不会真正联系 systemd 或 HTTPS。

目录汇总模式回归：`disk_space_scanner_smoke.py` 验证稀疏文件与硬链接仍计入总量，但输出不含文件名；32,010 个文件只保存一个目录，无临时 SQLite。`isolated_smoke.py --disk-space` 在私有数据库中验证 26,001 个子目录的分页与排序、未被筛选影响的本层占用、目录约束拒绝文件行；另在私有 schema 重演 0013–0016，验证旧流式/JSON 快照的目录 ID 与总量保留、失败任务保留及旧表/文件名清除。

七天使用历史：`isolated_smoke.py --usage-history` 使用私有 PostgreSQL/HTTPS，验证自然日边界、过期/未来数据排除、缓存采样去重、真实零值与缺失空白、消失 GPU 编号保留、序列化结果缓存和参数边界。在临时数据库短暂锁定历史表，检查历史查询限流/超时以及主监控健康接口仍然响应；不锁定或读取生产表。`web/tests/usageHistory.test.ts` 与 SSR 验证缺失/零值、GPU 选择与长时间轴日期显示。

清单缓存与性能回归：默认 HTTP smoke 覆盖协议 3 的完整/引用上报、缓存缺失与冲突拒绝、完整快照一致性、旧协议拒绝和重复重传。Rust 队列测试覆盖内容共享、不同版本、重启恢复及旧队列；真实 Agent 的丢缓存、采集开关和暂停恢复仍走原隔离入口。

可加 `--performance-baseline /tmp/server-before`，将已备份的旧服务二进制只读映射到隔离环境，比较每组 240 次相同采样比例的模拟上报。仅在私有 PostgreSQL 创建临时数据库，对比请求正文、WAL 生成、后端 CPU 时间和观测 RSS；另验证带真实数据的 schema-21 → 22 升级/回退、跨节点缓存隔离与缓存重建。包含旧 Agent 向新平台完整上报的兼容场景。它不是生产压力测试，不测 TLS 握手、所有采集子进程或整机磁盘写入；CPU 时间很短，受时钟精度和调度影响，不能用其百分比承诺生产收益。
