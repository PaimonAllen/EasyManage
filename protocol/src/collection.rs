//! Shared, bounded collection policy. Intervals are seconds, not browser refresh rates.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CollectionPolicy {
    /// None means a legacy peer/global policy; Some advertises per-node control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub users_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_enabled: Option<bool>,
    pub revision: u64,
    pub live_seconds: u64,
    pub report_seconds: u64,
    pub gpu_seconds: u64,
    pub disk_seconds: u64,
    pub extended_seconds: u64,
    pub permission_seconds: u64,
    pub node_seconds: u64,
    pub server_process_seconds: u64,
    pub platform_health_seconds: u64,
    pub gitea_seconds: u64,
}

// key, title, included observations, minimum, maximum, default
pub const COLLECTION_ITEMS: &[(&str, &str, &str, u64, u64, u64)] = &[
    (
        "live_seconds",
        "实时指标",
        "CPU 使用率、内存、网络吞吐；轻量通道，不写数据库",
        1,
        60,
        1,
    ),
    (
        "report_seconds",
        "完整上报 / 心跳",
        "最新快照、长期历史、负载、Swap、运行时间及 Agent 自身性能",
        5,
        30,
        10,
    ),
    (
        "gpu_seconds",
        "GPU 指标",
        "核心、显存、温度、功耗；失败后至少退避 60 秒",
        5,
        3600,
        10,
    ),
    (
        "disk_seconds",
        "磁盘容量与 I/O",
        "挂载点容量、设备读写速率；CPU / SSD 温度复用此周期且不低于 30 秒；不读写业务文件",
        10,
        3600,
        30,
    ),
    (
        "extended_seconds",
        "扩展清单",
        "用户与会话、块设备与分区、网卡/路由、Tailscale/ZeroTier、代理、软件/服务、安全、AI 与编程工具；共用一次扫描",
        30,
        3600,
        60,
    ),
    (
        "permission_seconds",
        "权限检查",
        "个人/共享/项目目录、sudo 与 root；只读检查",
        60,
        3600,
        300,
    ),
    (
        "node_seconds",
        "节点基础信息",
        "OS、内核、架构、CPU 型号与核心数",
        60,
        86400,
        300,
    ),
    (
        "server_process_seconds",
        "平台自身性能",
        "Server 全进程 CPU、RSS 与 I/O，包含平台监控自己的开销",
        5,
        300,
        15,
    ),
    (
        "platform_health_seconds",
        "平台与节点链路",
        "数据库可达、注册表、节点心跳、SSH TCP 端口；按需检查的缓存间隔",
        5,
        300,
        15,
    ),
    (
        "gitea_seconds",
        "Gitea 状态与账户",
        "本机 Gitea 健康、版本、可见账户；按需读取的缓存间隔",
        30,
        3600,
        60,
    ),
];

impl Default for CollectionPolicy {
    fn default() -> Self {
        Self {
            users_enabled: None,
            permissions_enabled: None,
            proxy_enabled: None,
            revision: 0,
            live_seconds: 1,
            report_seconds: 10,
            gpu_seconds: 10,
            disk_seconds: 30,
            extended_seconds: 60,
            permission_seconds: 300,
            node_seconds: 300,
            server_process_seconds: 15,
            platform_health_seconds: 15,
            gitea_seconds: 60,
        }
    }
}

impl CollectionPolicy {
    pub fn values(&self) -> [u64; 10] {
        [
            self.live_seconds,
            self.report_seconds,
            self.gpu_seconds,
            self.disk_seconds,
            self.extended_seconds,
            self.permission_seconds,
            self.node_seconds,
            self.server_process_seconds,
            self.platform_health_seconds,
            self.gitea_seconds,
        ]
    }

    pub fn validate(&self) -> Result<(), String> {
        for (value, (_, title, _, min, max, _)) in self.values().into_iter().zip(COLLECTION_ITEMS) {
            if !(*min..=*max).contains(&value) {
                return Err(format!("{title}必须为 {min}–{max} 秒的整数"));
            }
        }
        if self.revision > i64::MAX as u64 {
            return Err("配置版本超出范围".into());
        }
        if self.gpu_seconds < self.report_seconds || self.disk_seconds < self.report_seconds {
            return Err("GPU 与磁盘间隔不能短于完整上报间隔".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unsafe_and_incoherent_intervals() {
        assert!(CollectionPolicy::default().validate().is_ok());
        for invalid in [0, 61, u64::MAX] {
            assert!(
                CollectionPolicy {
                    live_seconds: invalid,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            CollectionPolicy {
                extended_seconds: 1,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            CollectionPolicy {
                permission_seconds: 1,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            CollectionPolicy {
                report_seconds: 30,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
}
