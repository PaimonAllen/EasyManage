# 测试约定

- `integration/`：Server、真实 Agent、数据库故障恢复、可信部署及强制重启 mock 回归，见 [隔离执行说明](integration/README.md)。
- `web/tests/`：前端纯逻辑与图表服务端渲染。
- Rust 模块内 `#[cfg(test)]`：协议、缓存、温度、权限、数据库配置与采样单元测试。

生产主机只编译测试，不直接运行。所有运行时测试进入独立网络/挂载命名空间，使用私有 tmpfs，遮蔽宿主 home、设备、sysfs、systemd/SSH socket；不能连接生产数据库或服务。测试资源全部在该隔离环境创建并清理。

Agent 原生后端的单元测试随实现放在 `agent/src/platform/linux/`，共用探测测试在 `platform/common/`。目录规范检查为 `scripts/check-repository.py`；无实现的 crates、打包和 C++ 占位已经移除。新平台必须增加原生与协议兼容测试，不能仅凭目录存在标记为支持。
