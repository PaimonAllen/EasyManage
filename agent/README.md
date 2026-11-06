# Agent 真实代码

这里仍是单个 Rust crate，不拆成没有实现的子 crate。`Cargo.toml` 构建唯一 `easymanage-agent` 二进制；当前已实现并验证的后端为 Linux。

```text
agent/
├── config/agent.env.example     安全配置模板
└── src/
    ├── main.rs                 程序启动、日志、TLS、目标平台构建保护
    ├── app.rs                  共用调度、注册、实时上报、离线队列发送
    ├── client.rs               HTTPS 平台客户端
    ├── config.rs               共用配置解析，原生路径交给平台层
    ├── store.rs                SQLite 身份、缓存与离线队列
    └── platform/
        ├── mod.rs              选择已实现的原生后端
        ├── contract.rs         PlatformCollector 统一接口
        ├── common/             限时固定命令与可选 NVIDIA 探测
        └── linux/              现有 Linux 指标、用户、网络、权限、硬件与路径
```

平台接口静态调用，不使用动态 trait 对象、不启动额外工作线程。现有 Linux 缓存、采集频率、Full / Basic、状态路径和协议字段保持不变。原来的 `agent/src/collector.rs` 已归位到 `src/platform/linux/collector.rs`，共用调度单独在 `app.rs`。

Windows/macOS 原生采集尚未实现；非 Linux 目标明确拒绝构建，不生成伪造正常指标的 Agent。新增后端需要实现统一接口、原生配置/状态路径、权限及缺失指标语义，再注册对应 cfg；详见 [跨平台说明](../docs/architecture.md)。不要为了预留创建空平台目录或复制一套客户端、队列和调度。

系统级服务/安装器在 `deploy/linux/system/`，普通用户模式在 `deploy/linux/user/`。`deploy/windows/` 与 `deploy/macos/` 仅放原生安装接入说明，并无可用安装器。实际配置和 SQLite 身份存仓库外，不能为目录整理迁移或重建；当前工作不更新生产节点。

低频清单（用户、权限、路由、设备与环境）通过协议 3 按摘要复用；服务端确认缓存后才省略正文。CPU/GPU/磁盘读数、运行权限与控制状态始终完整上报，采集频率保持原配置。软件缓存或清单缓存丢失、平台回退时，客户端完整重试一次，不重新注册。SQLite 队列按内容共享这些清单，但每份历史报告仍有独立的确认、版本和时间；旧格式队列可读取，回退旧程序前应排空新版队列或恢复一致备份。
