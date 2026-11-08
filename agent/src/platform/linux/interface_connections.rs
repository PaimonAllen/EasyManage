//! Read current connection labels; no Wi-Fi scan, saved profiles or credentials.
use crate::platform::common::command::read_command;
use easymanage_protocol::NetworkInterface;
use std::collections::HashMap;

const OUTPUT_LIMIT: u64 = 128 * 1024;

pub async fn enrich(interfaces: &mut [NetworkInterface]) {
    if interfaces.is_empty() {
        return;
    }
    let wifi_connected = interfaces
        .iter()
        .any(|item| item.kind == "wifi" && item.state == "up");
    let (connections, wifi) = tokio::join!(
        read_command(
            "nmcli",
            &[
                "--terse",
                "--escape",
                "yes",
                "--fields",
                "DEVICE,STATE,CONNECTION",
                "device",
                "status",
            ],
            OUTPUT_LIMIT,
        ),
        async {
            if wifi_connected {
                // Read only NetworkManager's existing AP cache. Never trigger a rescan.
                read_command(
                    "nmcli",
                    &[
                        "--terse",
                        "--escape",
                        "yes",
                        "--fields",
                        "IN-USE,SSID,DEVICE",
                        "device",
                        "wifi",
                        "list",
                        "--rescan",
                        "no",
                    ],
                    OUTPUT_LIMIT,
                )
                .await
            } else {
                Ok(String::new())
            }
        }
    );
    apply_names(
        interfaces,
        &active_connections(&connections.unwrap_or_default()),
        &active_wifi(&wifi.unwrap_or_default()),
    );

    // NetworkManager is optional. Query only associated interfaces missing an SSID,
    // with the existing 3-second command timeout and at most 16 concurrent helpers.
    let mut pending = tokio::task::JoinSet::new();
    for item in interfaces
        .iter()
        .filter(|item| item.kind == "wifi" && item.state == "up" && item.wifi_ssid.is_none())
        .take(16)
    {
        let name = item.name.clone();
        pending.spawn(async move {
            let output = read_command("iw", &["dev", &name, "link"], 16 * 1024).await;
            (name, output.ok().and_then(|value| iw_ssid(&value)))
        });
    }
    while let Some(result) = pending.join_next().await {
        if let Ok((name, Some(ssid))) = result
            && let Some(item) = interfaces.iter_mut().find(|item| item.name == name)
        {
            item.wifi_ssid = Some(ssid);
        }
    }
}

fn apply_names(
    interfaces: &mut [NetworkInterface],
    connections: &HashMap<String, String>,
    wifi: &HashMap<String, String>,
) {
    for item in interfaces {
        // Do not retain the last saved connection name on disconnected devices.
        item.connection_name = if matches!(item.state.as_str(), "up" | "unknown") {
            connections.get(&item.name).cloned()
        } else {
            None
        };
        item.wifi_ssid = if item.kind == "wifi" && item.state == "up" {
            wifi.get(&item.name).cloned()
        } else {
            None
        };
    }
}

fn active_connections(data: &str) -> HashMap<String, String> {
    data.lines()
        .take(512)
        .filter_map(|line| {
            let fields = nmcli_fields(line)?;
            if fields.len() != 3
                || !matches!(fields[1].as_str(), "connected" | "connected (externally)")
            {
                return None;
            }
            Some((fields[0].clone(), label(&fields[2])?))
        })
        .collect()
}

fn active_wifi(data: &str) -> HashMap<String, String> {
    data.lines()
        .take(4096)
        .filter_map(|line| {
            let fields = nmcli_fields(line)?;
            if fields.len() != 3 || fields[0] != "*" {
                return None;
            }
            Some((fields[2].clone(), label(&fields[1])?))
        })
        .collect()
}

// nmcli's terse output escapes colon and backslash, including within names.
fn nmcli_fields(line: &str) -> Option<Vec<String>> {
    let mut fields = vec![String::new()];
    let mut escaped = false;
    for character in line.chars() {
        if escaped {
            if !matches!(character, ':' | '\\') {
                return None;
            }
            fields.last_mut()?.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == ':' {
            fields.push(String::new());
        } else {
            fields.last_mut()?.push(character);
        }
    }
    (!escaped).then_some(fields)
}

fn label(value: &str) -> Option<String> {
    if value.trim().is_empty()
        || value == "--"
        || value.len() > 256
        || value.chars().any(char::is_control)
    {
        return None;
    }
    Some(value.to_owned())
}

fn iw_ssid(data: &str) -> Option<String> {
    if !data.lines().next()?.starts_with("Connected to ") {
        return None;
    }
    data.lines()
        .find_map(|line| label(line.trim_start().strip_prefix("SSID: ")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_active_connections_and_associated_ssids_are_read() {
        let connections = active_connections(
            "wlan0:connected:Office\\:Main\\\\WiFi\nwlan1:disconnected:Old\neno1:connected:办公网\ntun0:connected (externally):vpn\nwlan2:connecting (prepare):New\n",
        );
        assert_eq!(connections.len(), 3);
        assert_eq!(connections["wlan0"], "Office:Main\\WiFi");
        assert_eq!(connections["eno1"], "办公网");
        let wifi = active_wifi(" :Nearby:wlan0\n*:Actual\\:SSID:wlan0\n*:Second:wlan1\n");
        assert_eq!(wifi["wlan0"], "Actual:SSID");
        assert_eq!(wifi.len(), 2);
        assert!(nmcli_fields("bad\\").is_none());
        assert!(nmcli_fields("bad\\nname").is_none());
    }

    #[test]
    fn disconnected_and_old_agent_interfaces_do_not_invent_connection_names() {
        let mut interfaces: Vec<NetworkInterface> = serde_json::from_value(serde_json::json!([
            {"name":"wlan0","kind":"wifi","state":"up","addresses":[]},
            {"name":"wlan1","kind":"wifi","state":"down","addresses":[]},
            {"name":"eno1","kind":"ethernet","state":"up","addresses":[]}
        ]))
        .unwrap();
        let connections = active_connections(
            "wlan0:connected:Profile\nwlan1:connected:Stale\neno1:connected:Wired\n",
        );
        let wifi = active_wifi("*:SSID:wlan0\n*:Stale:wlan1\n*:Wrong:eno1\n");
        apply_names(&mut interfaces, &connections, &wifi);
        assert_eq!(interfaces[0].connection_name.as_deref(), Some("Profile"));
        assert_eq!(interfaces[0].wifi_ssid.as_deref(), Some("SSID"));
        assert!(interfaces[1].connection_name.is_none() && interfaces[1].wifi_ssid.is_none());
        assert_eq!(interfaces[2].connection_name.as_deref(), Some("Wired"));
        assert!(interfaces[2].wifi_ssid.is_none());
        apply_names(&mut interfaces, &HashMap::new(), &HashMap::new());
        assert!(
            interfaces
                .iter()
                .all(|item| item.connection_name.is_none() && item.wifi_ssid.is_none())
        );
    }

    #[test]
    fn iw_fallback_requires_a_real_association() {
        assert_eq!(
            iw_ssid(
                "Connected to aa:bb:cc:dd:ee:ff (on wlan0)\n\tSSID: Office WiFi\n\tfreq: 5180\n"
            )
            .as_deref(),
            Some("Office WiFi")
        );
        assert!(iw_ssid("Not connected.\n\tSSID: Old\n").is_none());
        assert!(iw_ssid("Connected to aa:bb:cc:dd:ee:ff\n\tSSID: \n").is_none());
    }
}
