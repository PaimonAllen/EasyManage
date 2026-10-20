// Run only under the documented disposable bwrap environment, not production.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createServer } from "vite";
import react from "@vitejs/plugin-react";

const [hostNet, hostMount] = process.argv.slice(2);
const { readlink } = await import("node:fs/promises");
assert.ok(hostNet && hostMount, "Original namespace IDs are required");
assert.notEqual(await readlink("/proc/self/ns/net"), hostNet, "Refusing host network namespace");
assert.notEqual(await readlink("/proc/self/ns/mnt"), hostMount, "Refusing host mount namespace");
const server = await createServer({
  configFile: false, root: fileURLToPath(new URL("..", import.meta.url)),
  plugins: [react()], cacheDir: "/tmp/easymanage-chart-vite-cache",
  server: { middlewareMode: true, watch: null },
});
try {
  const { AdminCredentialFields }=await server.ssrLoadModule("/src/components/AdminCredentials.tsx");
  const auth={ready:true,loading:false,available:true,mode:"saved",username:"fixture-admin",password:"fixture-password-not-for-render",
    key:"fixture-private-key-not-for-render",passphrase:"fixture-phrase-not-for-render",sudoPassword:"fixture-sudo-not-for-render",error:"",setMode:()=>{}};
  const savedAdmin=renderToStaticMarkup(React.createElement(AdminCredentialFields,{auth,disabled:false}));
  assert.ok(savedAdmin.includes("使用平台已加密保存的凭据")&&savedAdmin.includes("清除已保存的管理员凭据"));
  assert.ok(!savedAdmin.includes("fixture-")&&!savedAdmin.includes("localStorage"));
  const keyAdmin=renderToStaticMarkup(React.createElement(AdminCredentialFields,{auth:{...auth,mode:"key",password:"",passphrase:"",sudoPassword:""},disabled:true}));
  assert.ok(keyAdmin.includes('type="file"')&&keyAdmin.includes("12 KiB")&&keyAdmin.includes('disabled=""'));
  assert.ok(!keyAdmin.includes(auth.key));
  const loadingAdmin=renderToStaticMarkup(React.createElement(AdminCredentialFields,{auth:{...auth,loading:true},disabled:false}));
  assert.ok(loadingAdmin.includes("正在检查已保存的授权")&&!loadingAdmin.includes("fixture-"));
  console.log("PASS administrator credential forms support saved/password/key modes without revealing saved secrets");
  const { UserCollectionToggle } = await server.ssrLoadModule("/src/pages/Users.tsx");
  const userToggle=(extra)=>renderToStaticMarkup(React.createElement(UserCollectionToggle,{
    hostname:"Fixture node",enabled:true,applied:true,online:true,busy:false,onChange:()=>{},...extra,
  }));
  assert.match(userToggle({}),/role="switch"[^>]*aria-label="Fixture node 用户状态采集"[^>]*checked/);
  assert.ok(userToggle({enabled:false,applied:false}).includes("Agent 已停止采集"));
  assert.ok(userToggle({enabled:false,applied:true}).includes("等待 Agent 确认"));
  assert.ok(userToggle({enabled:false,applied:undefined}).includes("需更新 Agent"));
  assert.ok(userToggle({enabled:false,online:false}).includes("上线后应用"));
  assert.ok(userToggle({busy:true}).includes('disabled=""'));
  assert.ok(userToggle({enabled:undefined}).includes('disabled=""'));
  console.log("PASS per-node user collection switch shows confirmed, pending, offline and legacy states");
  const { NodeCollectionToggle } = await server.ssrLoadModule("/src/components/NodeCollectionControl.tsx");
  const permissionToggle=renderToStaticMarkup(React.createElement(NodeCollectionToggle,{
    hostname:"Permission fixture",label:"权限检查采集",enabled:false,applied:false,online:true,busy:false,onChange:()=>{},
  }));
  assert.ok(permissionToggle.includes('role="switch"')&&permissionToggle.includes('aria-label="Permission fixture 权限检查采集"'));
  assert.ok(permissionToggle.includes("Agent 已停止采集"));
  const { PermissionChecks } = await server.ssrLoadModule("/src/pages/PermissionChecks.tsx");
  const permissionPage=renderToStaticMarkup(React.createElement(PermissionChecks,{nodes:[
    {id:"permission-one",hostname:"First machine",online:true,metrics:null},
    {id:"permission-two",hostname:"Second machine",online:false,metrics:null},
  ]}));
  assert.equal((permissionPage.match(/role="switch"/g)??[]).length,2);
  assert.ok(permissionPage.includes("First machine 权限检查采集")&&permissionPage.includes("Second machine 权限检查采集"));
  console.log("PASS permission page has independent accessible per-machine collection switches");
  const { GlobalNetwork,proxyMonitoringVisible }=await server.ssrLoadModule("/src/pages/GlobalNetwork.tsx");
  const proxyFixture={id:"proxy-one",hostname:"Proxy machine",online:true,metrics:{
    collection_policy:{proxy_enabled:false},proxy_services:[{name:"mihomo",username:"fixture",uid:1000,pid:54321,listen_addresses:["tcp://127.0.0.1:23456"]}],
  }};
  assert.equal(proxyMonitoringVisible(proxyFixture,true),false);
  assert.equal(proxyMonitoringVisible({...proxyFixture,metrics:{collection_policy:{proxy_enabled:true}}},false),false);
  assert.equal(proxyMonitoringVisible({...proxyFixture,metrics:{collection_policy:{proxy_enabled:true}}},true),true);
  assert.equal(proxyMonitoringVisible({...proxyFixture,metrics:{}},undefined),true);
  const proxyPage=renderToStaticMarkup(React.createElement(GlobalNetwork,{nodes:[proxyFixture,{id:"proxy-two",hostname:"Other proxy machine",online:false,metrics:null}]}));
  assert.equal((proxyPage.match(/role="switch"/g)??[]).length,2);
  assert.ok(proxyPage.includes("Proxy machine 代理监控采集")&&proxyPage.includes("代理监控采集已关闭"));
  assert.ok(!proxyPage.includes("54321")&&!proxyPage.includes("23456")&&!proxyPage.includes("mihomo"));
  assert.ok(proxyPage.includes("不会停止实际代理"));
  const oldProxyPlatform=renderToStaticMarkup(React.createElement(NodeCollectionToggle,{
    hostname:"Legacy platform",label:"代理监控采集",enabled:undefined,applied:true,online:true,busy:false,unavailable:true,onChange:()=>{},
  }));
  assert.ok(oldProxyPlatform.includes("平台需更新")&&oldProxyPlatform.includes('disabled=""'));
  console.log("PASS per-machine proxy switches exclude disabled snapshots and listeners without stopping actual proxy services");
  const { NetworkResourceRows,NetworkResources }=await server.ssrLoadModule("/src/components/NetworkResources.tsx");
  const netRow={group:"agent_live",started:5,completed:4,failed:1,in_flight:1,open_connections:0,received_bytes:12,submitted_bytes:24};
  const netActivity={process_id:42,started_at:"2026-01-01T00:00:00Z",collected_at:"2026-01-01T00:01:00Z",groups:[netRow]};
  const netHtml=renderToStaticMarkup(React.createElement(NetworkResourceRows,{name:"Network fixture",activity:netActivity}));
  assert.ok(netHtml.includes("12 B")&&netHtml.includes("24 B")&&netHtml.includes("实时指标"));
  const unknownNet=renderToStaticMarkup(React.createElement(NetworkResourceRows,{name:"Legacy fixture",activity:null}));
  assert.ok(unknownNet.includes("不按 0 处理"));
  const offlineNet=renderToStaticMarkup(React.createElement(NetworkResourceRows,{name:"Offline fixture",activity:netActivity,online:false}));
  assert.ok(offlineNet.includes("离线历史")&&!/\d[\d. ]*(?:[KMG]i?B|B)\/s/.test(offlineNet));
  const netPanel=renderToStaticMarkup(React.createElement(NetworkResources));
  assert.ok(netPanel.includes("重启清零")&&netPanel.includes("不可相加")&&netPanel.includes("不记录 URL"));
  console.log("PASS network resource rows use component counters, preserve unknown/offline states and disclose non-additive layers");
  const { NodeCharts } = await server.ssrLoadModule("/src/components/NodeCharts.tsx");
  const end = Date.now();
  const at = new Date(end - 1000).toISOString();
  const metrics = {
    collected_at: at, gpu_collected_at: at, disk_collected_at: at,
    temperatures: { collected_at: at, cpu_celsius: 51.5, ssds: [{ device: "nvme0", temperature_celsius: 42 }] },
    cpu_usage_percent: 10, load_average_one: 0.2,
    memory_used_bytes: 1024, memory_total_bytes: 4096, uptime_seconds: 100,
    disks: [{ name: "nvme0n1", read_bytes_per_second: 100, written_bytes_per_second: 200 }],
    networks: [], gpus: [{ name: "Fixture GPU", index: 0, utilization_percent: 25,
      memory_used_bytes: 8 * 1024 ** 3, memory_total_bytes: 32 * 1024 ** 3, temperature_celsius: 60 }],
  };
  const props = { node: { metrics, online: true, cpu_logical_cores: 4 }, liveSamples: [], checkedAt: end };
  const html = renderToStaticMarkup(React.createElement(NodeCharts, props));
  assert.equal((html.match(/class="history-chart"/g) ?? []).length, 5);
  assert.equal((html.match(/stroke="var\(--chart-blue\)"/g) ?? []).length, 3);
  assert.equal((html.match(/class="history-chart__thermal-max"/g) ?? []).length, 3);
  assert.equal((html.match(/class="history-chart__thermal-min"/g) ?? []).length, 3);
  assert.ok(html.includes('class="history-chart__thermal-max">65°C'));
  assert.ok(html.includes('class="history-chart__thermal-min">55°C'));
  assert.ok(!html.includes('>150°C</span>'));
  assert.ok(html.includes('d="M599.0,80.0" fill="none" stroke="var(--chart-blue)"'));
  const earlier = new Date(end - 11_000).toISOString();
  const legacyHistory = { points: [{ ...metrics, collected_at: earlier, gpu_collected_at: earlier,
    temperatures: null, gpus: [{ ...metrics.gpus[0], temperature_celsius: null }] }] };
  const withSnapshots = renderToStaticMarkup(React.createElement(NodeCharts, {
    ...props, history: legacyHistory,
    temperatureSnapshots: [{ ...metrics, collected_at: earlier, gpu_collected_at: earlier,
      gpus: [{ ...metrics.gpus[0], temperature_celsius: 59 }] }],
  }));
  assert.ok(withSnapshots.includes('d="M589.0,96.0 L599.0,80.0" fill="none" stroke="var(--chart-blue)"'),
    "two actual overview sensor samples should draw the GPU temperature curve even with legacy history API");
  const hotMetrics = { ...metrics,
    temperatures: { ...metrics.temperatures, cpu_celsius: 71, ssds: [{ device: "nvme0", temperature_celsius: 70 }] },
    gpus: [{ ...metrics.gpus[0], temperature_celsius: 80 }],
  };
  const hotSingle = renderToStaticMarkup(React.createElement(NodeCharts, {
    ...props, node: { ...props.node, metrics: hotMetrics },
  }));
  assert.equal((hotSingle.match(/class="history-chart__temperature history-chart__temperature--hot"/g) ?? []).length, 2);
  assert.equal((hotSingle.match(/r="8" fill="var\(--chart-hot\)"/g) ?? []).length, 2);
  assert.equal((hotSingle.match(/r="8" fill="var\(--chart-blue\)"/g) ?? []).length, 1,
    "exactly 70°C remains blue");
  const hotHistory = { points: [{ ...metrics, collected_at: earlier, gpu_collected_at: earlier,
    temperatures: { ...metrics.temperatures, collected_at: earlier, cpu_celsius: 69,
      ssds: [{ device: "nvme0", temperature_celsius: 69 }] },
    gpus: [{ ...metrics.gpus[0], temperature_celsius: 60 }] }] };
  const hotCurve = renderToStaticMarkup(React.createElement(NodeCharts, {
    ...props, node: { ...props.node, metrics: hotMetrics }, history: hotHistory,
  }));
  assert.equal((hotCurve.match(/stroke="var\(--chart-hot\)"/g) ?? []).length, 2,
    "only CPU and GPU series cross the threshold");
  assert.ok(hotCurve.includes('d="M589.0,120.0 L599.0,40.0" fill="none" stroke="var(--chart-blue)"'));
  assert.match(hotCurve, /d="M589\.0,120\.0 L599\.0,40\.0"[^>]*stroke="var\(--chart-hot\)"[^>]*clip-path="url\(#[^)]+\)"/);
  assert.equal((hotCurve.match(/clipPathUnits="userSpaceOnUse"><rect x="0" y="0" width="600" height="80"/g) ?? []).length, 2,
    "red is clipped at the interpolated 70°C crossing, not applied to the whole line");
  const hotClipIds = [...hotCurve.matchAll(/<clipPath id="([^"]+)"/g)].map(match => match[1]);
  assert.equal(new Set(hotClipIds).size, hotClipIds.length, "multiple charts must have independent clip paths");
  assert.ok(html.includes("51.5°C") && html.includes("42.0°C") && html.includes("60.0°C"));
  assert.ok(html.includes("SSD 温度") && html.includes("nvme0: 42.0°C"));
  assert.ok(html.includes("history-chart__max--primary"));
  const headings = [...html.matchAll(/<div class="history-chart__heading">(.*?)<div class="history-chart__plot">/g)].map(match => match[1]);
  assert.equal(headings.length, 5);
  for (const index of [0, 2, 3]) {
    const heading = headings[index];
    const metaEnd = heading.indexOf('</div>');
    const thermalReading = heading.indexOf('class="history-chart__temperature"');
    assert.ok(thermalReading > metaEnd, "Temperature must not displace header metadata");
    assert.ok(heading.endsWith('</span></strong></div>'), "Temperature must follow the primary readings");
  }
  assert.ok(headings[2].includes('8.0/32.0 GiB'));
  assert.ok(headings[2].indexOf('8.0/32.0 GiB') < headings[2].indexOf('class="history-chart__temperature"'), "GPU temperature must follow memory capacity");
  const css = await readFile(new URL("../src/styles.css", import.meta.url), "utf8");
  assert.ok(css.includes(".history-chart__legend { gap: 3px 5px; flex-wrap: wrap;"));
  assert.ok(css.includes('.history-chart__headline > .metric-dual-headline { display: contents; }'));
  assert.ok(css.includes('flex-basis: 100%; white-space: normal;'));
  assert.ok(css.includes('.history-chart__headline > .history-chart__temperature--hot { color: var(--chart-hot); }'));
  assert.ok(css.includes(':root[data-theme="light"] .history-chart__headline > .history-chart__temperature--hot { color: var(--chart-hot); }'));
  const legacy = { ...metrics, temperatures: null, gpus: [] };
  const missing = renderToStaticMarkup(React.createElement(NodeCharts, {
    ...props, node: { ...props.node, metrics: legacy },
  }));
  assert.equal((missing.match(/stroke="var\(--chart-blue\)"/g) ?? []).length, 0);
  assert.equal((missing.match(/>温度 —<\/span>/g) ?? []).length, 3);
  assert.ok(!missing.includes(">0.0°C<"));
  console.log("PASS five-chart rendering, three blue temperature curves with independent dynamic bounds, compact legend and legacy/missing-sensor display");
  const { BlockDeviceMap } = await server.ssrLoadModule("/src/components/BlockDeviceMap.tsx");
  const models = ["ST2000DM005-2CW102", "ST4000NM0035-1V4107", "ST4000DM004-2U91", "Unknown HDD", "Samsung SSD 990 EVO Plus 1TB"];
  const devices = models.map((model, index) => ({ name: `fixture${index}`, path: `/dev/fixture${index}`,
    kind: "disk", model, rotational: index !== 4, size_bytes: 1000, mount_points: [], children: [] }));
  const partition = { ...devices[0], name: "fixture0p1", path: "/dev/fixture0p1", kind: "part", model: null,
    filesystem: "ext4", start_bytes: 0, size_bytes: 500, mount_points: ["/data"], children: [] };
  const storage = renderToStaticMarkup(React.createElement(BlockDeviceMap, {
    devices: [{ ...devices[0], children: [partition] }, ...devices.slice(1)], disks: [],
  }));
  assert.equal((storage.match(/storage-recording-badge--smr/g) ?? []).length, 2);
  assert.equal((storage.match(/storage-recording-badge--cmr/g) ?? []).length, 1);
  assert.equal((storage.match(/storage-recording-badge--unknown/g) ?? []).length, 1);
  assert.equal((storage.match(/aria-expanded="false"/g) ?? []).length, 4);
  assert.ok(storage.includes("SMR 叠瓦盘") && storage.includes("CMR 垂直盘") && storage.includes("记录方式未确认"));
  const recordingBadges = [...storage.matchAll(/<button[^>]*storage-recording-badge--(smr|cmr|unknown)[^>]*>(.*?)<\/button>/g)];
  assert.equal(recordingBadges.length, 4);
  for (const [, kind, content] of recordingBadges) {
    assert.equal(content.includes("ⓘ"), kind !== "cmr", "green CMR label has no notice icon; other labels retain theirs");
  }
  assert.equal((storage.match(/<details class="storage-device-details">/g) ?? []).length, 5);
  assert.equal((storage.match(/<details class="storage-device-details" open/g) ?? []).length, 0,
    "partition trees start collapsed independently for each disk");
  assert.match(storage, /<summary>设备与分区详情<span>· 1 个下属设备<\/span><\/summary><div class="block-device-tree">/);
  const graphPosition = storage.indexOf('aria-label="/dev/fixture0 的分区容量图"');
  const legendPosition = storage.indexOf('class="partition-map__legend"');
  const detailsPosition = storage.indexOf('<details class="storage-device-details">');
  assert.ok(graphPosition >= 0 && legendPosition > graphPosition && detailsPosition > legendPosition,
    "partition graph and legend stay visible outside the collapsed tree");
  const partialUsage = renderToStaticMarkup(React.createElement(BlockDeviceMap, {
    devices: [{ ...devices[0], children: [partition] }],
    disks: [{ name: "/dev/fixture0p1", mount_point: "/data", total_bytes: 500, available_bytes: 250 }],
  }));
  assert.match(partialUsage, /class="partition-map__used" style="width:50%"/);
  assert.match(partialUsage, /class="partition-map__usage-boundary" style="left:50%" aria-hidden="true"/,
    "partial usage draws a solid dark cap and narrow groove at the used/free boundary");
  assert.ok(css.includes('--partition-cap-width: 5px;') && css.includes('--partition-divider-width: 2px;')
    && css.includes('width: calc(var(--partition-cap-width) + var(--partition-divider-width));')
    && css.includes('var(--partition-stripe-color) 0 var(--partition-cap-width),')
    && css.includes('border-right: var(--partition-divider-width) solid var(--surface);')
    && css.includes('border-left: var(--partition-divider-width) solid var(--surface);'));
  const nearFull = renderToStaticMarkup(React.createElement(BlockDeviceMap, {
    devices: [{ ...devices[0], children: [{ ...partition, size_bytes: 997 }] }], disks: [],
  }));
  assert.match(nearFull, /class="partition-map__trailing-free" style="width:0\.3%" aria-hidden="true"/,
    "tiny trailing unallocated space gets a visible region and separator");
  assert.ok(nearFull.includes("实际容量以图例数字为准"));
  const completelyAllocated = renderToStaticMarkup(React.createElement(BlockDeviceMap, {
    devices: [{ ...devices[0], children: [{ ...partition, size_bytes: 1000 }] }], disks: [],
  }));
  assert.equal((completelyAllocated.match(/partition-map__trailing-free/g) ?? []).length, 0,
    "no free area is invented when the last partition reaches the disk end");
  assert.ok(css.includes('.partition-map__trailing-free {'));
  assert.ok(css.includes('min-width: 8px;'));
  assert.ok(css.includes('.storage-device-details > summary:focus-visible'));
  assert.ok(css.includes('.storage-device-details[open] > summary::before'));
  console.log("PASS storage recording badges: two SMR, one CMR, unknown model, SSD exclusion and accessible detail toggles");
  const { AgentManagement,updateEligible } = await server.ssrLoadModule("/src/pages/AgentManagement.tsx");
  const pendingPauseUpgrade={online:false,monitoring_paused:true,last_seen:new Date().toISOString(),metrics:{privilege:{mode:"full",uid:0},agent_control:null}};
  const availableFull={update_available:true,update_state:"available"};
  assert.equal(updateEligible(pendingPauseUpgrade,availableFull),true);
  assert.equal(updateEligible({...pendingPauseUpgrade,last_seen:"2020-01-01T00:00:00Z"},availableFull),false);
  assert.equal(updateEligible({...pendingPauseUpgrade,metrics:{...pendingPauseUpgrade.metrics,agent_control:{revision:1,paused:true}}},availableFull),false);
  const emergency = renderToStaticMarkup(React.createElement(AgentManagement, {
    nodes: [], targets: [{node_id:"offline-fixture",hostname:"offline-node",host:"192.0.2.1",port:22,restart_available:true}],
    onRefresh:()=>{},onElevate:()=>{},
  }));
  assert.ok(emergency.includes("强制重启平台") && emergency.includes("offline-node"));
  assert.match(emergency, /<button[^>]*type="button"[^>]*>强制重启 Agent<\/button>/);
  assert.ok(!/<button[^>]*disabled[^>]*>强制重启 Agent<\/button>/.test(emergency),
    "offline configured targets remain operable when node snapshots are unavailable");
  console.log("PASS emergency restart controls remain visible and Agent action enabled without node data");
  const { Storage } = await server.ssrLoadModule("/src/pages/Storage.tsx");
  const mounted = (mount_point, available_bytes) => ({
    name: "/dev/nvme0n1p5", mount_point, file_system: "ext4", total_bytes: 1000,
    available_bytes, read_bytes_per_second: 0, written_bytes_per_second: 0,
  });
  const storagePage = renderToStaticMarkup(React.createElement(Storage, { nodes: [{
    id: "storage-fixture", hostname: "storage-fixture", online: true,
    metrics: { ...metrics, swap_used_bytes: 0, swap_total_bytes: 0,
      block_devices: [{ ...devices[0], children: [partition] }, devices[1]],
      collection_issues: [{ component: "disk", message: "Synthetic read-only fixture" }],
      disks: [mounted("/", 99), mounted("/var/lib/easymanage-agent", 99),
        mounted("/data/at-90", 100), mounted("/data/normal", 500),
        mounted("/data/low", 810), mounted("/data/at-20", 800),
        mounted("/data/medium", 450), mounted("/data/invalid", 1100)],
    },
  }] }));
  assert.match(storagePage, /<span class="storage-snapshot-ages"><span class="snapshot-age[^"]*"[^>]*>磁盘采集于 [\s\S]*?<\/span><span class="snapshot-age[^"]*"[^>]*>尚未收到清单快照<\/span><\/span>/,
    "disk and inventory snapshot times share one inline row");
  assert.ok(css.includes('.storage-snapshot-ages {\n  display: inline-flex;'));
  const topologyHeading = storagePage.indexOf("磁盘与分区拓扑");
  const expandAll = storagePage.indexOf(">全部展开</button>");
  const collapseAll = storagePage.indexOf(">全部折叠</button>");
  const topologyMap = storagePage.indexOf('aria-label="/dev/fixture0 的分区容量图"');
  assert.ok(topologyHeading >= 0 && expandAll > topologyHeading && collapseAll > expandAll && topologyMap > collapseAll,
    "global controls appear at the right of the topology heading, before the always-visible map");
  assert.match(storagePage, /<div class="storage-topology-toolbar__actions"><button[^>]*type="button">全部展开<\/button><button[^>]*type="button">全部折叠<\/button><\/div>/);
  assert.equal((storagePage.match(/<details class="storage-device-details">/g) ?? []).length, 2);
  assert.ok(css.includes('.storage-topology-toolbar__actions { display: flex; gap: 6px;'));
  assert.ok(css.includes('.storage-topology-toolbar .section-title { flex: 0 0 auto;'),
    "expand and collapse controls stay next to the topology title");
  assert.match(storagePage, /<details class="storage-mounted-section" open=""><summary class="storage-mounted-toggle"><h3 class="section-title">已挂载文件系统与 I\/O<\/h3><\/summary><div class="table-scroll"><table class="storage-table">/);
  assert.match(storagePage, /<\/table><\/div><details class="collection-notes">[\s\S]*?<\/details><\/details>/,
    "table and storage collection notes belong to the collapsible section");
  assert.ok(css.includes('.storage-mounted-section:not([open]) .storage-mounted-toggle::after'));
  assert.ok(css.includes('.storage-mounted-toggle:focus-visible'));
  assert.equal((storagePage.match(/class="meter__fill meter__fill--storage"/g) ?? []).length, 8);
  assert.equal((storagePage.match(/class="number storage-table__low-space"/g) ?? []).length, 2);
  assert.equal((storagePage.match(/class="meter__warning" role="img"/g) ?? []).length, 2);
  assert.equal((storagePage.match(/⚠/g) ?? []).length, 2);
  assert.ok(storagePage.includes('--meter-accent:var(--storage-green)'));
  assert.ok(storagePage.includes('--meter-accent:color-mix(in srgb, var(--storage-orange-light) 50.0%, var(--storage-orange-deep) 50.0%)'));
  assert.ok(storagePage.includes('--meter-accent:var(--chart-hot)'));
  const invalidRow = storagePage.match(/<tr><td><strong>\/data\/invalid<\/strong>[\s\S]*?<\/tr>/)?.[0];
  assert.ok(invalidRow && invalidRow.includes('<strong>—</strong>') && !invalidRow.includes('meter__warning'));
  console.log("PASS mounted filesystem usage: low green, 20–90 graded orange, >90 red with warning, invalid unknown, duplicate mount points consistent");
  const { StorageCell } = await server.ssrLoadModule("/src/pages/PlatformHealth.tsx");
  const storageWindow = { total_bytes: { average: 4096, maximum: 8192, samples: 2 },
    last_sample_at: at, latest: { collected_at: at, binary_bytes: 1024, web_bytes: 1024,
      database_bytes: 2048, state_bytes: null, total_bytes: 4096 } };
  const platformStorage = renderToStaticMarkup(React.createElement(StorageCell, { storage: storageWindow, server: true }));
  assert.ok(platformStorage.includes("均值 4 KiB") && platformStorage.includes("最大 8 KiB"));
  assert.ok(platformStorage.includes("数据库 2 KiB") && platformStorage.includes("网页 1 KiB"));
  const agentStorage = renderToStaticMarkup(React.createElement(StorageCell, {
    storage: { ...storageWindow, latest: { ...storageWindow.latest, state_bytes: 2048, database_bytes: null, web_bytes: null } },
    server: false,
  }));
  assert.ok(agentStorage.includes("SQLite 2 KiB") && !agentStorage.includes("数据库"));
  console.log("PASS long-interval platform and Agent storage footprint cells show history and component breakdown");
  const { HardwareDetailsContent } = await server.ssrLoadModule("/src/components/HardwareDetailsDialog.tsx");
  const hardware = { collected_at: at, processors: ["CPU fixture"], motherboard: ["Board fixture"],
    memory: ["2 × 32 GiB DDR5"], graphics: ["GPU fixture"], displays: [], disks: ["SSD fixture"],
    audio: ["Audio fixture"], network: ["NIC fixture"], notes: ["EDID unavailable"] };
  const hardwarePanel = renderToStaticMarkup(React.createElement(HardwareDetailsContent, {
    node: { hostname: "fixture", online: true, metrics: { hardware_details: hardware } },
  }));
  for (const label of ["处理器", "主板", "内存", "显卡", "显示器", "磁盘", "声卡", "网卡"]) {
    assert.ok(hardwarePanel.includes(`<dt>${label}</dt>`));
  }
  assert.ok(hardwarePanel.includes("每天更新一次") && hardwarePanel.includes("未检测到或暂不可读取")
    && hardwarePanel.includes("EDID unavailable") && hardwarePanel.includes("2 × 32 GiB DDR5"));
  const oldHardware = renderToStaticMarkup(React.createElement(HardwareDetailsContent, { node: { online: true, metrics: {} } }));
  assert.ok(oldHardware.includes("尚未收到设备详情"));
  const staleHardware = renderToStaticMarkup(React.createElement(HardwareDetailsContent, {
    node: { online: false, metrics: { hardware_details: hardware } },
  }));
  assert.ok(staleHardware.includes("历史快照"));
  console.log("PASS daily hardware details categories, missing hardware, old Agents and offline timestamps");
  const { AiTools } = await server.ssrLoadModule("/src/pages/AiTools.tsx");
  const aiPage = renderToStaticMarkup(React.createElement(AiTools, { nodes: [{
    id: "ai-fixture", hostname: "ai-host", online: true, metrics: {
      collected_at: at, inventory_collected_at: at,
      ai_tools: [
        { name: "codex", category: "assistant", executable_paths: ["/home/fixture/.npm-global/bin/codex"], processes: [], service_units: [] },
        { name: "claude", category: "assistant", executable_paths: ["/home/fixture/.local/bin/claude"], processes: [], service_units: [] },
        { name: "ollama", category: "runtime", executable_paths: ["/runtime-fixture/ollama"], processes: [{ pid: 1, uid: 1000, username: "fixture" }], service_units: ["ollama.service"] },
      ],
      collection_issues: [{ component: "ai_tools", message: "AI 安装目录有 1 处权限不足" }],
    },
  }] }));
  assert.ok(aiPage.includes("/home/fixture/.npm-global/bin/codex") && aiPage.includes("已发现可执行文件"));
  assert.ok(aiPage.includes("扫描不完整") && aiPage.includes("权限不足") && aiPage.includes("NVM / FNM / mise"));
  assert.ok(aiPage.includes("Claude Code") && aiPage.includes("Agent 工具"));
  assert.ok(!aiPage.includes("/runtime-fixture/ollama") && !aiPage.includes("ollama.service"));
  assert.ok(aiPage.includes("已发现安装的工具</strong><span>2</span>"));
  assert.ok(aiPage.includes("运行进程</strong><span>0</span>") && aiPage.includes("systemd 服务</strong><span>0</span>"));
  const oldAi = renderToStaticMarkup(React.createElement(AiTools, { nodes: [{ id: "old-ai", hostname: "old", online: false, metrics: {} }] }));
  assert.ok(oldAi.includes("尚未收到新版 Agent 的 AI 工具清单"));
  console.log("PASS expanded multi-user AI inventory paths and incomplete-scan reasons, with legacy Agent fallback");
  const { MachineManagement, MachineMonitoringButton, machineMonitoringLabel, MachineSshDialog, RedeployMachineDialog, SshAuthenticationFields, AgentConnectionSettings, AgentDeploymentFields } = await server.ssrLoadModule("/src/components/MachineManagement.tsx");
  const pausedMachine={monitoring_paused:true,monitoring_revision:1,monitoring_applied_revision:1,monitoring_supported:true,control_seen_at:new Date().toISOString()};
  assert.equal(machineMonitoringLabel(pausedMachine),"已暂停 · 轻量控制在线");
  assert.equal(machineMonitoringLabel({...pausedMachine,monitoring_supported:false}),"暂停待确认 · 需更新 Agent");
  assert.equal(machineMonitoringLabel({...pausedMachine,monitoring_applied_revision:0}),"暂停待确认");
  assert.equal(machineMonitoringLabel({...pausedMachine,control_seen_at:"2020-01-01T00:00:00Z"}),"已暂停 · 控制连接离线");
  assert.match(machineMonitoringLabel({...pausedMachine,monitoring_paused:false,monitoring_revision:2}),/恢复待确认/);
  assert.equal(machineMonitoringLabel({...pausedMachine,monitoring_paused:false}),"已恢复 · 等待新指标");
  const monitoringButton=(machine,busy=false)=>renderToStaticMarkup(React.createElement(MachineMonitoringButton,{machine:{hostname:"Paused fixture",...machine},busy,onToggle:()=>{}}));
  assert.ok(monitoringButton(pausedMachine).includes('aria-label="Paused fixture 恢复监控"'));
  assert.ok(monitoringButton({...pausedMachine,monitoring_paused:false}).includes('aria-label="Paused fixture 暂停监控"'));
  assert.ok(monitoringButton({}).includes("需先更新平台后端")&&monitoringButton({}).includes('disabled=""'));
  assert.ok(monitoringButton(pausedMachine,true).includes('disabled=""'));
  console.log("PASS monitoring pause distinguishes confirmed, pending, unsupported, disconnected and resume states");
  const { NodeStatus }=await server.ssrLoadModule("/src/components/NodeStatus.tsx");
  const pausedStatus=(node)=>renderToStaticMarkup(React.createElement(NodeStatus,{node}));
  assert.ok(pausedStatus({monitoring_paused:true,monitoring_revision:1,monitoring_applied_revision:0,online:true}).includes("暂停待确认"));
  const confirmedStatus=pausedStatus({monitoring_paused:true,monitoring_revision:1,monitoring_applied_revision:1,online:false});
  assert.ok(confirmedStatus.includes("监控已暂停")&&confirmedStatus.includes("status--paused")&&!confirmedStatus.includes("status--offline"));
  const management=renderToStaticMarkup(React.createElement(MachineManagement,{onRefresh:()=>{}}));
  assert.ok(management.includes("机器管理") && management.includes("添加机器") && management.includes("软删除保留历史和身份"));
  assert.ok(management.includes("查看受管机器与已删除机器") && management.includes("disabled"));
  console.log("PASS overview machine management entry, safe default Basic, archive list and unavailable-candidate protection");
  const connectionSettings = renderToStaticMarkup(React.createElement(AgentConnectionSettings,{url:"https://platform.example:8443",busy:false,onChange:()=>{}}));
  assert.ok(connectionSettings.includes("自动写入 Agent 配置") && connectionSettings.includes('value="https://platform.example:8443"'));
  assert.ok(connectionSettings.includes('value="auto" selected=""') && connectionSettings.includes("最多 3 条 SSH"));
  assert.ok(connectionSettings.includes("不为每次采集重新登录") && connectionSettings.includes("需管理员允许 SSH 回环端口转发"));
  const sshSettings=renderToStaticMarkup(React.createElement(AgentConnectionSettings,{url:"https://platform.example:8443",mode:"ssh",busy:false,onChange:()=>{},onModeChange:()=>{}}));
  assert.ok(sshSettings.includes('value="ssh" selected=""') && sshSettings.includes("隧道继续校验 CA/证书"));
  assert.match(connectionSettings, /<details><summary>高级网络设置（通常无需修改）<\/summary>/);
  assert.ok(!connectionSettings.includes("<details open") && connectionSettings.includes("不是目标机器地址"));
  const busyConnection = renderToStaticMarkup(React.createElement(AgentConnectionSettings,{url:"https://platform.example:8443",busy:true,onChange:()=>{}}));
  assert.match(busyConnection, /<input[^>]*disabled=""/);
  console.log("PASS automatic Agent connection settings with collapsed override and deployment-time protection");
  const modeProps={onChange:()=>{},busy:false,username:"tester",sudoRef:{current:null}};
  const basicMode=renderToStaticMarkup(React.createElement(AgentDeploymentFields,{...modeProps,mode:"basic"}));
  const fullMode=renderToStaticMarkup(React.createElement(AgentDeploymentFields,{...modeProps,mode:"full"}));
  const rootMode=renderToStaticMarkup(React.createElement(AgentDeploymentFields,{...modeProps,mode:"full",username:"root"}));
  assert.ok(basicMode.includes('value="basic" selected=""') && basicMode.includes("均在该用户 ~/.easymanage 内") && !basicMode.includes('type="password"'));
  assert.ok(fullMode.includes('value="full" selected=""') && fullMode.includes("/var/lib/easymanage-agent") && fullMode.includes('type="password"'));
  assert.ok(!rootMode.includes('type="password"'));
  console.log("PASS Basic/Full deployment choices, home-only Basic storage, root/sudo notices and masked sudo input");
  const fieldProps = {onMethodChange:()=>{},busy:false,required:true,passwordRef:{current:null},keyRef:{current:null},passphraseRef:{current:null}};
  const passwordFields = renderToStaticMarkup(React.createElement(SshAuthenticationFields,{...fieldProps,method:"password"}));
  const keyFields = renderToStaticMarkup(React.createElement(SshAuthenticationFields,{...fieldProps,method:"private_key"}));
  assert.ok(passwordFields.includes("鉴权方式") && passwordFields.includes('type="password"') && !passwordFields.includes('type="file"'));
  assert.ok(keyFields.includes('type="file"') && keyFields.includes('type="password"') && keyFields.includes("私钥口令"));
  assert.ok(keyFields.includes("平台仅保存加密副本") && !keyFields.includes("textarea"));
  console.log("PASS password/key authentication selector, masked password/passphrase and non-displaying private-key upload");
  const savedDialog=renderToStaticMarkup(React.createElement(MachineSshDialog,{machine:{node_id:"fixture",hostname:"Fixture SSH",username:"fixture",host:"10.222.0.9",port:22,saved_credentials_usable:true},onClose:()=>{},onDone:()=>{}}));
  assert.ok(savedDialog.includes("使用平台已加密保存")&&!savedDialog.includes('type="password"')&&!savedDialog.includes('type="file"'));
  assert.ok(savedDialog.includes("AES-256-GCM")&&savedDialog.includes("每 60 秒")&&savedDialog.includes("最多 3 次"));
  assert.ok(savedDialog.includes("删除平台保存的凭据")&&savedDialog.includes("手动断开不会自动恢复"));
  console.log("PASS saved SSH credential reuse without secret fields, encrypted storage notices and bounded retry controls");
  const redeployProps={machine:{node_id:"original-fixture-uuid",hostname:"Original SSH fixture",username:"tester",host:"10.222.0.9",port:2222,deployment_mode:"full",connection_mode:"ssh",saved_credentials_usable:true},onClose:()=>{},onStarted:()=>{}};
  const redeployDialog=renderToStaticMarkup(React.createElement(RedeployMachineDialog,redeployProps));
  assert.ok(redeployDialog.includes("重新部署 Agent")&&redeployDialog.includes("original-fixture-uuid")&&redeployDialog.includes("保留原机器 ID、历史、名称和备注"));
  assert.ok(redeployDialog.includes("保持原权限：管理员 Full")&&redeployDialog.includes('value="ssh" selected=""'));
  assert.ok(!redeployDialog.includes('type="password"')&&!redeployDialog.includes('type="file"'));
  assert.ok(redeployDialog.includes("拒绝覆盖")&&redeployDialog.includes("确认原 Agent 已卸载")&&redeployDialog.includes("disabled"));
  const manualRedeploy=renderToStaticMarkup(React.createElement(RedeployMachineDialog,{...redeployProps,machine:{...redeployProps.machine,saved_credentials_usable:false}}));
  assert.ok(manualRedeploy.includes('type="password"')&&manualRedeploy.includes("sudo 密码"));
  console.log("PASS same-UUID redeployment confirmation, preserved Full privilege, original target and encrypted-credential reuse");
  const agentManagement = renderToStaticMarkup(React.createElement(AgentManagement, {nodes: [], targets: [], onRefresh: () => {}, onElevate: () => {}}));
  assert.ok(agentManagement.includes("平台自身更新") && agentManagement.includes("更新平台并重启"));
  assert.ok(agentManagement.includes("强制重启平台") && agentManagement.includes("失败回退文件，不回退数据库迁移"));
  assert.ok(/disabled=""[^>]*title="正在检查平台候选"/.test(agentManagement));
  console.log("PASS separate platform update entry, unavailable-candidate protection and migration risk notice");
} finally {
  await server.close();
}
