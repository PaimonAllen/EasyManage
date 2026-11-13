//! Linux executable locations; parsing and timeout handling are shared.
use easymanage_protocol::OverlayNetworkStatus;
use std::path::Path;
pub async fn collect() -> Vec<OverlayNetworkStatus> {
    let tailscale = if Path::new("/usr/bin/tailscale").is_file() {
        "/usr/bin/tailscale"
    } else {
        "tailscale"
    };
    let zerotier = if Path::new("/usr/sbin/zerotier-cli").is_file() {
        "/usr/sbin/zerotier-cli"
    } else {
        "zerotier-cli"
    };
    crate::platform::common::overlay_networks::collect_with(tailscale, zerotier, &[]).await
}
