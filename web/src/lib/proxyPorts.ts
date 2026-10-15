import type { ProxyService } from "../types/monitoring";

export interface ProxyObservation {
  nodeId: string;
  hostname: string;
  service: ProxyService;
}

interface Listener {
  nodeId: string;
  hostname: string;
  pid: number;
  username: string;
  name: string;
  address: string;
  protocol: "tcp" | "udp";
  family: 4 | 6;
  host: string;
  port: number;
}

export interface ProxyPortConflict {
  hostname: string;
  protocol: "tcp" | "udp";
  port: number;
  left: Listener;
  right: Listener;
}

export interface ProxyPortAnalysis {
  conflicts: ProxyPortConflict[];
  dualStackUncertain: number;
  invalidAddresses: number;
  observedListeners: number;
}

function parseListener(address: string): Pick<Listener, "protocol" | "family" | "host" | "port"> | null {
  const match = /^(tcp|udp):\/\/(.+)$/.exec(address);
  if (!match) return null;
  const endpoint = match[2];
  const family: 4 | 6 = endpoint.startsWith("[") ? 6 : 4;
  let host: string;
  let portText: string;
  if (family === 6) {
    const end = endpoint.indexOf("]");
    if (end < 2 || endpoint[end + 1] !== ":") return null;
    host = endpoint.slice(1, end).toLowerCase();
    portText = endpoint.slice(end + 2);
  } else {
    const colon = endpoint.lastIndexOf(":");
    if (colon < 1 || endpoint.slice(0, colon).includes(":")) return null;
    host = endpoint.slice(0, colon);
    portText = endpoint.slice(colon + 1);
  }
  if (family === 4) {
    const octets = host.split(".");
    if (octets.length !== 4 || octets.some((part) => !/^\d{1,3}$/.test(part) || Number(part) > 255)) return null;
  } else if (!/^[0-9a-f:.]+$/.test(host)) return null;
  if (!/^\d{1,5}$/.test(portText)) return null;
  const port = Number(portText);
  if (port < 1 || port > 65535) return null;
  return { protocol: match[1] as "tcp" | "udp", family, host, port };
}

/** Detect overlapping *observed* proxy listeners, not attempted/failed binds. */
export function analyzeProxyPorts(observations: ProxyObservation[]): ProxyPortAnalysis {
  const groups = new Map<string, Listener[]>();
  const dualStack = new Map<string, { v4: Set<number>; v6Wildcard: Set<number> }>();
  let invalidAddresses = 0;
  let observedListeners = 0;

  for (const { nodeId, hostname, service } of observations) {
    for (const address of service.listen_addresses) {
      const parsed = parseListener(address);
      if (!parsed) {
        invalidAddresses += 1;
        continue;
      }
      observedListeners += 1;
      const listener: Listener = {
        ...parsed, nodeId, hostname, address,
        pid: service.pid, username: service.username, name: service.name,
      };
      const key = `${nodeId}|${parsed.protocol}|${parsed.family}|${parsed.port}`;
      const group = groups.get(key) ?? [];
      group.push(listener);
      groups.set(key, group);

      const crossKey = `${nodeId}|${parsed.protocol}|${parsed.port}`;
      const cross = dualStack.get(crossKey) ?? { v4: new Set<number>(), v6Wildcard: new Set<number>() };
      if (parsed.family === 4) cross.v4.add(service.pid);
      else if (parsed.host === "::" || parsed.host.startsWith("::ffff:")) cross.v6Wildcard.add(service.pid);
      dualStack.set(crossKey, cross);
    }
  }

  const conflicts: ProxyPortConflict[] = [];
  for (const group of groups.values()) {
    const byHost = new Map<string, { first: Listener; another?: Listener }>();
    for (const listener of group) {
      const bucket = byHost.get(listener.host);
      if (!bucket) byHost.set(listener.host, { first: listener });
      else if (bucket.first.pid !== listener.pid && !bucket.another) bucket.another = listener;
    }
    let pair: [Listener, Listener] | undefined;
    for (const bucket of byHost.values()) {
      if (bucket.another) {
        pair = [bucket.first, bucket.another];
        break;
      }
    }
    if (!pair) {
      const wildcard = group[0].family === 4 ? "0.0.0.0" : "::";
      const allInterfaces = byHost.get(wildcard)?.first;
      if (allInterfaces) {
        for (const [host, bucket] of byHost) {
          if (host !== wildcard && bucket.first.pid !== allInterfaces.pid) {
            pair = [allInterfaces, bucket.first];
            break;
          }
        }
      }
    }
    if (pair) conflicts.push({
      hostname: pair[0].hostname, protocol: pair[0].protocol,
      port: pair[0].port, left: pair[0], right: pair[1],
    });
  }

  let dualStackUncertain = 0;
  for (const { v4, v6Wildcard } of dualStack.values()) {
    if (v4.size && v6Wildcard.size && new Set([...v4, ...v6Wildcard]).size > 1) {
      dualStackUncertain += 1;
    }
  }
  return { conflicts, dualStackUncertain, invalidAddresses, observedListeners };
}
