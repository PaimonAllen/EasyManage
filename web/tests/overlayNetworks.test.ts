import assert from "node:assert/strict";
import { test } from "node:test";
import { localOverlayNetworks } from "../src/lib/overlayNetworks.ts";
import type { NetworkInterface, OverlayNetworkStatus } from "../src/types/monitoring.ts";

const nic = (name: string, kind: string, addresses: string[]): NetworkInterface => ({
  name, kind, addresses, state:"up", mac_address:null, mtu:1500, vlan_id:null, master:null,
});
const status: OverlayNetworkStatus = {provider:"zerotier",state:"healthy",detail:"fixture",networks:[
  {network_id:"0123456789abcdef",name:"First network",state:"OK",interface:"zt-one"},
  {network_id:"fedcba9876543210",name:"Second network",state:"ACCESS_DENIED",interface:"zt-two"},
]};

test("local overlay addresses match exact network interfaces, including legacy interface kinds", () => {
  const interfaces = [nic("zt-one","virtual",["192.0.2.1/24"]),nic("zt-two","zerotier",["198.51.100.1/24"]),
    nic("tailscale0","tailscale",["100.100.100.100/32","fd7a:115c:a1e0::1/128"]),nic("eth0","ethernet",["203.0.113.1/24"])];
  const rows=localOverlayNetworks("zerotier",status,interfaces);
  assert.equal(rows.length,2);
  assert.deepEqual(rows.map(row=>row.interface?.addresses),[["192.0.2.1/24"],["198.51.100.1/24"]]);
  assert.equal(rows[1].membership?.state,"ACCESS_DENIED");
  const tailscale=localOverlayNetworks("tailscale",undefined,interfaces);
  assert.deepEqual(tailscale.map(row=>row.interface?.addresses),[["100.100.100.100/32","fd7a:115c:a1e0::1/128"]]);
  assert.equal(tailscale[0].membership,null);
});

test("missing CLI or interface evidence stays unknown instead of assigning another network's IP", () => {
  const local=nic("zt-one","zerotier",["192.0.2.1/24"]);
  const missing={...status,networks:[{...status.networks[0],interface:null}]};
  const rows=localOverlayNetworks("zerotier",missing,[local]);
  assert.equal(rows[0].interface,null);
  assert.equal(rows[1].membership,null);
  assert.equal(rows[1].interface,local);
  assert.deepEqual(localOverlayNetworks("zerotier",undefined,[]),[]);
  assert.equal(localOverlayNetworks("tailscale",status,[local]).length,0);
});
