import type { NetworkInterface, OverlayNetworkStatus } from "../types/monitoring";

export function interfaceConnectionNames(item: NetworkInterface, overlays: OverlayNetworkStatus[]) {
  const names: { label: string; name: string }[] = [];
  const add = (label: string, name?: string | null) => {
    if (name?.trim() && !names.some((entry) => entry.name === name)) {
      names.push({ label, name });
    }
  };
  // Never show an old connection label as active after a link goes down.
  if (!["up", "unknown"].includes(item.state.toLowerCase())) return names;
  if (item.kind === "wifi" && item.state.toLowerCase() === "up") add("Wi-Fi", item.wifi_ssid);
  if (item.kind === "tailscale" || item.kind === "zerotier") {
    for (const status of overlays.filter((status) => status.provider === item.kind)) {
      for (const network of status.networks) {
        if (network.interface === item.name) add("网络", network.name);
      }
    }
  }
  if (item.connection_name !== item.name) add("连接", item.connection_name);
  return names;
}
