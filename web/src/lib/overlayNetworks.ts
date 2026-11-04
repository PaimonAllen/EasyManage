import type { NetworkInterface, OverlayMembership, OverlayNetworkStatus } from "../types/monitoring.ts";

export interface LocalOverlayNetwork {
  membership: OverlayMembership | null;
  interface: NetworkInterface | null;
}

// A membership's exact interface identifies its local IPs; never associate all
// ZeroTier interfaces with every network or include addresses from another NIC.
export function localOverlayNetworks(
  provider: OverlayNetworkStatus["provider"],
  status: OverlayNetworkStatus | undefined,
  interfaces: NetworkInterface[],
): LocalOverlayNetwork[] {
  const memberships = status?.provider === provider ? status.networks : [];
  const matched = new Set<string>();
  const rows: LocalOverlayNetwork[] = memberships.map((membership) => {
    const local = interfaces.find((item) => item.name === membership.interface) ?? null;
    if (local) matched.add(local.name);
    return { membership, interface: local };
  });
  // CLI authorization may be unavailable while the kernel still reports a
  // virtual interface. Show its IPs without inventing membership/health.
  for (const local of interfaces) {
    if (local.kind === provider && !matched.has(local.name)) {
      rows.push({ membership: null, interface: local });
      matched.add(local.name);
    }
  }
  return rows;
}
