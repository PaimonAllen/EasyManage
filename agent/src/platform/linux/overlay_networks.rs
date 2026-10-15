use crate::platform::common::command::read_command;
use easymanage_protocol::{OverlayMembership, OverlayNetworkStatus};
use serde_json::Value;
use std::io;
use std::net::IpAddr;
use std::path::Path;

const CLI_OUTPUT_LIMIT: u64 = 256 * 1024;

pub async fn collect() -> Vec<OverlayNetworkStatus> {
    let tailscale_program = if Path::new("/usr/bin/tailscale").is_file() {
        "/usr/bin/tailscale"
    } else {
        "tailscale"
    };
    let zerotier_program = if Path::new("/usr/sbin/zerotier-cli").is_file() {
        "/usr/sbin/zerotier-cli"
    } else {
        "zerotier-cli"
    };
    let (tailscale, zerotier_info, zerotier_networks) = tokio::join!(
        read_command(
            tailscale_program,
            &["status", "--json", "--peers=false"],
            CLI_OUTPUT_LIMIT
        ),
        read_command(zerotier_program, &["-j", "info"], CLI_OUTPUT_LIMIT),
        read_command(zerotier_program, &["-j", "listnetworks"], CLI_OUTPUT_LIMIT),
    );
    vec![
        match tailscale {
            Ok(output) => parse_tailscale(&output),
            Err(error) => command_error("tailscale", &error),
        },
        match zerotier_info {
            Ok(info) => parse_zerotier(&info, zerotier_networks.as_deref().ok()),
            Err(error) => command_error("zerotier", &error),
        },
    ]
}

fn status(provider: &str, state: &str, detail: &str) -> OverlayNetworkStatus {
    OverlayNetworkStatus {
        provider: provider.to_owned(),
        state: state.to_owned(),
        detail: detail.to_owned(),
        networks: Vec::new(),
    }
}

fn command_error(provider: &str, error: &io::Error) -> OverlayNetworkStatus {
    if error.kind() == io::ErrorKind::NotFound {
        return status(provider, "not_installed", "未找到该服务的 CLI");
    }
    let detail = if provider == "zerotier" {
        "CLI 无法读取状态；Basic 用户可能缺少 ZeroTier 授权，请使用 Full 权限核验"
    } else {
        "CLI 无法读取状态；可能是权限不足、服务未运行或命令超时"
    };
    status(provider, "unknown", detail)
}

fn parse_tailscale(output: &str) -> OverlayNetworkStatus {
    let Ok(value) = serde_json::from_str::<Value>(output) else {
        return status("tailscale", "unknown", "CLI 状态 JSON 无法解析");
    };
    let Some(backend) = value.get("BackendState").and_then(Value::as_str) else {
        return status("tailscale", "unknown", "CLI 未返回 BackendState");
    };
    match backend {
        "Running" => {
            let Some(online) = value.pointer("/Self/Online").and_then(Value::as_bool) else {
                return status("tailscale", "unknown", "CLI 未返回本机在线状态");
            };
            let Some(addresses) = value
                .pointer("/Self/TailscaleIPs")
                .and_then(Value::as_array)
            else {
                return status("tailscale", "unknown", "CLI 未返回本机组网地址");
            };
            if !addresses.iter().any(|address| {
                address
                    .as_str()
                    .is_some_and(|address| address.parse::<IpAddr>().is_ok())
            }) {
                return status("tailscale", "not_joined", "守护进程运行，但暂未加入网络");
            }
            if !online {
                return status("tailscale", "offline", "守护进程运行，但本机未在线");
            }
            let health_warnings = value
                .get("Health")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            if health_warnings > 0 {
                status(
                    "tailscale",
                    "healthy",
                    &format!("守护进程运行且已加入网络；CLI 另有 {health_warnings} 项警告"),
                )
            } else {
                status("tailscale", "healthy", "守护进程运行且已加入网络")
            }
        }
        "NeedsLogin" => status(
            "tailscale",
            "not_joined",
            "守护进程运行，暂未登录或加入网络",
        ),
        "NeedsMachineAuth" => status("tailscale", "not_joined", "守护进程运行，等待组网授权"),
        "Stopped" => status("tailscale", "offline", "Tailscale 已停止"),
        "Starting" => status("tailscale", "degraded", "Tailscale 正在启动"),
        _ => status("tailscale", "unknown", "CLI 返回未识别的运行状态"),
    }
}

fn parse_zerotier(info: &str, networks_output: Option<&str>) -> OverlayNetworkStatus {
    let Ok(value) = serde_json::from_str::<Value>(info) else {
        return status("zerotier", "unknown", "CLI 节点状态 JSON 无法解析");
    };
    let Some(online) = value.get("online").and_then(Value::as_bool) else {
        return status("zerotier", "unknown", "CLI 未返回节点在线状态");
    };
    let tcp_fallback = value
        .get("tcpFallbackActive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let Some(networks_output) = networks_output else {
        return status("zerotier", "unknown", "节点在线，但 CLI 网络列表不可读取");
    };
    let Ok(networks_value) = serde_json::from_str::<Value>(networks_output) else {
        return status("zerotier", "unknown", "CLI 网络列表 JSON 无法解析");
    };
    let Some(networks) = networks_value.as_array() else {
        return status("zerotier", "unknown", "CLI 网络列表格式不正确");
    };
    if networks.len() > 64 {
        return status("zerotier", "unknown", "网络数量超过安全采集上限");
    }
    let mut memberships = Vec::with_capacity(networks.len());
    for network in networks {
        let (Some(network_id), Some(state)) = (
            network
                .get("nwid")
                .or_else(|| network.get("id"))
                .and_then(Value::as_str),
            network.get("status").and_then(Value::as_str),
        ) else {
            return status("zerotier", "unknown", "CLI 网络列表缺少 ID 或状态");
        };
        if network_id.len() != 16
            || !network_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || state.is_empty()
            || state.len() > 64
        {
            return status("zerotier", "unknown", "CLI 网络列表包含无效 ID 或状态");
        }
        memberships.push(OverlayMembership {
            network_id: network_id.to_owned(),
            name: network
                .get("name")
                .and_then(Value::as_str)
                .map(|name| name.chars().take(80).collect()),
            state: state.to_owned(),
            interface: network
                .get("portDeviceName")
                .and_then(Value::as_str)
                .map(|name| name.chars().take(32).collect()),
        });
    }
    let healthy_networks = memberships
        .iter()
        .filter(|network| network.state == "OK")
        .count();
    let mut result = if healthy_networks == 0 {
        status("zerotier", "not_joined", "守护进程运行，但暂未加入可用网络")
    } else if !online {
        status("zerotier", "offline", "已加入网络，但节点未连接上游")
    } else if healthy_networks != memberships.len() {
        status(
            "zerotier",
            "healthy",
            &format!(
                "守护进程运行，{healthy_networks}/{} 个网络状态 OK；其余网络尚未就绪",
                memberships.len()
            ),
        )
    } else if tcp_fallback {
        status("zerotier", "healthy", "已加入网络，当前使用 TCP 回退")
    } else {
        status(
            "zerotier",
            "healthy",
            &format!("守护进程运行，{} 个网络状态 OK", memberships.len()),
        )
    };
    result.networks = memberships;
    result
}

#[cfg(test)]
mod tests {
    use super::{parse_tailscale, parse_zerotier};

    #[test]
    fn tailscale_requires_running_online_and_joined_network() {
        assert_eq!(
            parse_tailscale(r#"{"BackendState":"Running","Self":{"Online":true,"TailscaleIPs":["100.100.100.100"]},"Health":[]}"#)
                .state,
            "healthy"
        );
        assert_eq!(
            parse_tailscale(
                r#"{"BackendState":"Running","Self":{"Online":true,"TailscaleIPs":["100.100.100.100"]},"Health":["warning"]}"#
            )
            .state,
            "healthy"
        );
        assert_eq!(
            parse_tailscale(r#"{"BackendState":"Running","Self":{"Online":false,"TailscaleIPs":["100.100.100.100"]},"Health":[]}"#)
                .state,
            "offline"
        );
        assert_eq!(
            parse_tailscale(
                r#"{"BackendState":"Running","Self":{"Online":true,"TailscaleIPs":[]}}"#
            )
            .state,
            "not_joined"
        );
        assert_eq!(
            parse_tailscale(r#"{"BackendState":"NeedsLogin"}"#).state,
            "not_joined"
        );
        assert_eq!(parse_tailscale("{} ").state, "unknown");
    }

    #[test]
    fn zerotier_requires_node_and_each_joined_network_to_be_ok() {
        let info = r#"{"online":true,"tcpFallbackActive":false}"#;
        let good =
            r#"[{"nwid":"0123456789abcdef","name":"team","status":"OK","portDeviceName":"zt1"}]"#;
        assert_eq!(parse_zerotier(info, Some(good)).state, "healthy");
        assert_eq!(parse_zerotier(info, None).state, "unknown");
        assert_eq!(parse_zerotier(info, Some("[]")).state, "not_joined");
        assert_eq!(
            parse_zerotier(info, Some(&good.replace("\"OK\"", "\"ACCESS_DENIED\""))).state,
            "not_joined"
        );
        let mixed = format!(
            "[{},{{\"nwid\":\"fedcba9876543210\",\"status\":\"ACCESS_DENIED\"}}]",
            &good[1..good.len() - 1]
        );
        assert_eq!(parse_zerotier(info, Some(&mixed)).state, "healthy");
        assert_eq!(
            parse_zerotier(r#"{"online":true,"tcpFallbackActive":true}"#, Some(good)).state,
            "healthy"
        );
        assert_eq!(
            parse_zerotier(r#"{"online":false,"tcpFallbackActive":false}"#, Some("[]")).state,
            "not_joined"
        );
        assert_eq!(
            parse_zerotier(r#"{"online":false,"tcpFallbackActive":false}"#, Some(good)).state,
            "offline"
        );
    }
}
