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
  const {GiteaDependencyChecks,GiteaAccountSummary}=await server.ssrLoadModule('/src/pages/Gitea.tsx');
  const failedGitea=renderToStaticMarkup(React.createElement(GiteaDependencyChecks,{checks:{
    'database:ping':[{status:'fail',time:'2026-11-05T06:56:37Z'}],'cache:ping':[{status:'pass'}],
  }}));
  assert.ok(failedGitea.includes('数据库连接')&&failedGitea.includes('缓存连接')&&failedGitea.includes('失败')&&failedGitea.includes('正常'));
  const unavailableGitea=renderToStaticMarkup(React.createElement(GiteaAccountSummary,{accounts:{scope:'unavailable',users:[]}}));
  assert.equal((unavailableGitea.match(/>—<\/span>/g)??[]).length,4);
  assert.ok(unavailableGitea.includes('暂不可用')&&!unavailableGitea.includes('>0</span>'));
  const emptyGitea=renderToStaticMarkup(React.createElement(GiteaAccountSummary,{accounts:{scope:'public',users:[]}}));
  assert.equal((emptyGitea.match(/>0<\/span>/g)??[]).length,4);
  console.log('PASS Gitea failed dependencies stay visible and unavailable accounts are distinct from a verified empty list');
  const {MarkdownPreview}=await server.ssrLoadModule('/src/components/MarkdownPreview.tsx');
  const gpuMarkdown=await readFile(new URL('../../server/content/gpu-comparison.md',import.meta.url),'utf8');
  const gpuReference=renderToStaticMarkup(React.createElement(MarkdownPreview,{content:gpuMarkdown}));
  assert.equal((gpuReference.match(/<table>/g)??[]).length,2);
  assert.ok(gpuReference.includes('RTX PRO 6000 Blackwell')&&gpuReference.includes('Tesla V100')&&gpuReference.includes('FP64'));
  const unsafeMarkdown=renderToStaticMarkup(React.createElement(MarkdownPreview,{content:'# Safe\n\n<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>\n\n[bad](javascript:alert)\n\n[good](https://example.com)'}));
  assert.ok(!unsafeMarkdown.includes('<script')&&!unsafeMarkdown.includes('onerror=')&&!unsafeMarkdown.includes('href="javascript:'));
  assert.ok(unsafeMarkdown.includes('href="https://example.com"')&&unsafeMarkdown.includes('rel="noopener noreferrer"'));
  const GpuComparison=(await server.ssrLoadModule('/src/pages/GpuComparison.tsx')).default;
  const gpuEditor=renderToStaticMarkup(React.createElement(GpuComparison));
  assert.ok(gpuEditor.includes('GPU 性能对比')&&gpuEditor.includes('重新读取'));
  console.log('PASS GPU reference preserves supplied tables and Markdown formatting, excludes executable HTML/URLs and keeps links separate from the editor');
  const { Network, OverlayHealthCard, OverlayLocalDetails } = await server.ssrLoadModule("/src/pages/Network.tsx");
  const networkFixture = { id: "route-one", hostname: "Route machine", online: true, metrics: {
    inventory_collected_at: new Date().toISOString(), routes: [{ destination: "198.51.100.0/24", gateway: "198.51.100.1", interface: "fixture0", metric: 100 }],
  } };
  const routesPage = renderToStaticMarkup(React.createElement(Network, { nodes: [networkFixture, { ...networkFixture, id: "route-two", hostname: "Empty routes", metrics: { routes: [] } }] }));
  assert.equal((routesPage.match(/<details class="network-route-details"/g) ?? []).length, 2);
  assert.ok(routesPage.includes("路由 · 1 条") && routesPage.includes("路由 · 0 条") && routesPage.includes("198.51.100.0/24") && routesPage.includes("fixture0"));
  assert.ok(!/<details class="network-route-details"[^>]*\bopen(?:=|\s|>)/.test(routesPage));
  const overlayFixture={...networkFixture,metrics:{...networkFixture.metrics,
    overlay_networks:[{provider:"zerotier",state:"healthy",detail:"Fixture online",networks:[
      {network_id:"0123456789abcdef",name:"Team <script>fixture</script>",state:"OK",interface:"zt-one"},
      {network_id:"fedcba9876543210",name:"Second network",state:"ACCESS_DENIED",interface:"zt-two"}]}],
    interfaces:[{name:"zt-one",kind:"zerotier",state:"up",addresses:["192.0.2.1/24","2001:db8::1/64"],mtu:2800,vlan_id:null,master:null},
      {name:"zt-two",kind:"zerotier",state:"down",addresses:[],mtu:2800,vlan_id:null,master:null}]} };
  const overlayCard=renderToStaticMarkup(React.createElement(OverlayHealthCard,{node:overlayFixture,provider:"zerotier"}));
  assert.ok(overlayCard.includes('aria-expanded="false"')&&overlayCard.includes('aria-controls=')&&overlayCard.includes('>展开</button>')&&!overlayCard.includes('192.0.2.1/24'));
  const localOverlay=renderToStaticMarkup(React.createElement(OverlayLocalDetails,{node:overlayFixture,provider:"zerotier"}));
  assert.ok(localOverlay.includes("0123456789abcdef")&&localOverlay.includes("192.0.2.1/24")&&localOverlay.includes("2001:db8::1/64")&&localOverlay.includes("未上报 IP"));
  assert.ok(localOverlay.includes('overlay-health-card__state--degraded">ACCESS_DENIED')&&!localOverlay.includes('<script>fixture</script>')&&!localOverlay.includes('VLAN Tag'));
  const staleOverlay=renderToStaticMarkup(React.createElement(OverlayLocalDetails,{node:{...overlayFixture,online:false},provider:"zerotier"}));
  assert.ok(staleOverlay.includes("历史快照")&&staleOverlay.includes("历史 · OK")&&!staleOverlay.includes('overlay-health-card__state--healthy'));
  const unassignedOverlay=renderToStaticMarkup(React.createElement(OverlayLocalDetails,{node:{...overlayFixture,metrics:{...overlayFixture.metrics,overlay_networks:[]}},provider:"zerotier"}));
  assert.ok(unassignedOverlay.includes("192.0.2.1/24")&&unassignedOverlay.includes('overlay-health-card__state--unknown">未上报')&&!unassignedOverlay.includes('overlay-health-card__state--healthy'));
  console.log("PASS overlay cards default closed; local IPs, network IDs, per-network states and stale snapshots stay separate without new probes");
  const { Security } = await server.ssrLoadModule("/src/pages/Security.tsx");
  const securityFixture = { ...networkFixture, metrics: { inventory_collected_at: new Date().toISOString(),
    host_security: { collected_at: new Date(Date.now() - 1800_000).toISOString(), interval_seconds: 3600, apparmor_enabled: true, aslr_mode: 2, ptrace_scope: 1, kernel_lockdown: "integrity" },
  } };
  const securityPage = renderToStaticMarkup(React.createElement(Security, { nodes: [securityFixture] }));
  assert.ok(securityPage.includes("小时快照") && securityPage.includes("每小时一次") && !securityPage.includes("历史或未知") && !securityPage.includes("实时观测"));
  const legacySecurityPage = renderToStaticMarkup(React.createElement(Security, { nodes: [{ ...securityFixture, metrics: { ...securityFixture.metrics, host_security: { apparmor_enabled: true } } }] }));
  assert.ok(legacySecurityPage.includes("旧版采样") && legacySecurityPage.includes("更新 Agent 后") && !legacySecurityPage.includes("小时快照"));
  console.log("PASS routes collapse separately by machine and hourly security retains its own freshness with truthful legacy labels");
  const {SpaceVisualization,DiskSpaceExplorer}=await server.ssrLoadModule("/src/components/DiskSpaceExplorer.tsx");
  const diskTree=[
    {id:0,parent:null,name:"/fixture",kind:"directory",allocated:100,apparent:1000,files:2,directories:1,issue:null,incomplete:true,mtime:0},
    {id:1,parent:0,name:"large",kind:"directory",allocated:80,apparent:800,files:1,directories:0,issue:null,incomplete:false,mtime:0},
    {id:2,parent:0,name:"<script>fixture</script>",kind:"file",allocated:20,apparent:200,files:1,directories:0,issue:null,incomplete:false,mtime:0},
  ];
  const diskHtml=renderToStaticMarkup(React.createElement(SpaceVisualization,{result:{root:"/fixture",uid:1000,username:"fixture",status:"partial",reason:"time_limit",elapsed_seconds:60,filesystem:"ext4",visited:3,errors:0,skipped_mounts:0,symlinks:0,hardlinks:0,nodes:diskTree},onChoosePath:()=>{}}));
  assert.ok(diskHtml.includes("80.0%")&&diskHtml.includes("占上一层")&&diskHtml.includes("达到所选扫描时限"));
  assert.ok(diskHtml.includes("不代表完整目录")&&diskHtml.includes("实际磁盘占用")&&diskHtml.includes("矩形图"));
  assert.ok(!diskHtml.includes("<script>fixture</script>")&&diskHtml.includes("&lt;script&gt;"));
  console.log("PASS hierarchical disk usage, parent percentages, partial lower-bound notices, treemap and escaped filenames");
  const controls=renderToStaticMarkup(React.createElement(DiskSpaceExplorer,{nodes:[{id:'fixture',hostname:'fixture',online:true,metrics:null}]}));
  assert.equal((controls.match(/value="0" selected="">无限制/g)??[]).length,2);
  assert.ok(controls.includes('关闭页面后任务继续')&&controls.includes('仅保存目录大小、数量和层级'));
  assert.ok(controls.includes('扫描范围')&&controls.includes('全部本地磁盘')&&controls.includes('指定目录仅扫描所在文件系统'));
  assert.ok(controls.includes('<option value="disk">指定磁盘</option>'));
  const {PagedSpaceVisualization}=await server.ssrLoadModule('/src/components/PagedSpaceVisualization.tsx');
  const paged=renderToStaticMarkup(React.createElement(PagedSpaceVisualization,{nodeId:'fixture',scanId:'fixture',result:{root:'/fixture',uid:1000,username:'fixture',status:'cancelled',reason:'cancelled',visited:32011,errors:0,skipped_mounts:0,symlinks:0,hardlinks:0,nodes:[diskTree[0]],paged:true},onChoosePath:()=>{}}));
  assert.ok(paged.includes('手动取消')&&paged.includes('大小为已读取部分的下限')&&paged.includes('排序作用于全部子目录'));
  const storageStopped=renderToStaticMarkup(React.createElement(PagedSpaceVisualization,{nodeId:'fixture',scanId:'storage-low',result:{root:'/fixture',uid:1000,username:'fixture',status:'partial',reason:'platform_storage_low',visited:1200,errors:0,skipped_mounts:0,symlinks:0,hardlinks:0,nodes:[diskTree[0]],paged:true},onChoosePath:()=>{}}));
  assert.ok(storageStopped.includes('平台结果存储空间不足')&&storageStopped.includes('大小为已读取部分的下限'));
  const allLocal=renderToStaticMarkup(React.createElement(PagedSpaceVisualization,{nodeId:'fixture',scanId:'fixture-all',result:{root:'/',scope:'all_local',uid:0,username:'root',status:'complete',reason:'',visited:5,errors:0,skipped_mounts:2,symlinks:0,hardlinks:0,nodes:[diskTree[0]],paged:true,filesystems:[{id:1,path:'/',filesystem:'ext4',incomplete:false},{id:3,path:'/data/disk',filesystem:'ext4',incomplete:false}]},onChoosePath:()=>{}}));
  assert.ok(allLocal.includes('全部本地磁盘')&&allLocal.includes('/data/disk')&&allLocal.includes('绑定挂载去重'));
  console.log('PASS unlimited time/count defaults and paged snapshot partial-result rendering');
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
  assert.ok(html.includes('均频 — · 4 线程 · Load 0.20')&&!html.includes('均频 0.00 GHz'));
  const frequencyMetrics={...metrics,cpu_frequency:{collected_at:at,average_mhz:2800,minimum_mhz:800,maximum_mhz:4200,logical_cpus:4}};
  const withFrequency=renderToStaticMarkup(React.createElement(NodeCharts,{...props,node:{...props.node,metrics:frequencyMetrics}}));
  assert.ok(withFrequency.includes('均频 2.80 GHz · 4 线程 · Load 0.20')&&withFrequency.includes('0.80–4.20 GHz')&&withFrequency.includes('30 秒缓存'));
  const staleFrequency=renderToStaticMarkup(React.createElement(NodeCharts,{...props,node:{...props.node,metrics:{...frequencyMetrics,cpu_frequency:{...frequencyMetrics.cpu_frequency,collected_at:new Date(end-91_000).toISOString()}}}}));
  assert.ok(staleFrequency.includes('均频 — · 4 线程')&&!staleFrequency.includes('均频 2.80 GHz'));
  console.log('PASS cached CPU frequency appears before threads/load, with range tooltip and unknown/stale data left empty');
  assert.equal((html.match(/stroke="var\(--chart-blue\)"/g) ?? []).length, 3);
  assert.equal((html.match(/class="history-chart__thermal-max"/g) ?? []).length, 3);
  assert.equal((html.match(/class="history-chart__thermal-min"/g) ?? []).length, 3);
  assert.ok(html.includes('class="history-chart__thermal-max">95°C'));
  assert.ok(html.includes('class="history-chart__thermal-min">30°C'));
  assert.ok(!html.includes('>150°C</span>'));
  assert.ok(html.includes('d="M599.0,86.2" fill="none" stroke="var(--chart-blue)"'));
  const earlier = new Date(end - 11_000).toISOString();
  const legacyHistory = { points: [{ ...metrics, collected_at: earlier, gpu_collected_at: earlier,
    temperatures: null, gpus: [{ ...metrics.gpus[0], temperature_celsius: null }] }] };
  const withSnapshots = renderToStaticMarkup(React.createElement(NodeCharts, {
    ...props, history: legacyHistory,
    temperatureSnapshots: [{ ...metrics, collected_at: earlier, gpu_collected_at: earlier,
      gpus: [{ ...metrics.gpus[0], temperature_celsius: 59 }] }],
  }));
  assert.ok(withSnapshots.includes('d="M589.0,88.6 L599.0,86.2" fill="none" stroke="var(--chart-blue)"'),
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
  assert.ok(hotCurve.includes('d="M589.0,86.2 L594.0,61.5 L599.0,36.9" fill="none" stroke="var(--chart-blue)"'));
  assert.match(hotCurve, /d="M589\.0,86\.2 L594\.0,61\.5 L599\.0,36\.9"[^>]*stroke="var\(--chart-hot\)"[^>]*clip-path="url\(#[^)]+\)"/);
  assert.equal((hotCurve.match(/clipPathUnits="userSpaceOnUse"><rect x="0" y="0" width="600" height="61\.538/g) ?? []).length, 2,
    "red is clipped at the interpolated 70°C crossing, not applied to the whole line");
  const hotClipIds = [...hotCurve.matchAll(/<clipPath id="([^"]+)"/g)].map(match => match[1]);
  assert.equal(new Set(hotClipIds).size, hotClipIds.length, "multiple charts must have independent clip paths");
  const {HistoryChart}=await server.ssrLoadModule('/src/components/HistoryChart.tsx');
  const extremeTemperature=renderToStaticMarkup(React.createElement(HistoryChart,{
    title:'GPU',headline:'fixture',detail:'fixture',series:[],maxValue:100,maxLabel:'100%',windowEnd:new Date(end).toISOString(),
    temperature:{value:110,detail:'fixture',gapSeconds:35,points:[
      {at:new Date(end-20_000).toISOString(),value:60},{at:new Date(end-10_000).toISOString(),value:110},{at:new Date(end-1_000).toISOString(),value:90},
    ]},
  }));
  assert.match(extremeTemperature,/class="history-chart__temperature-above-range" d="M587\.0,0\.0 L590\.0,0\.0 L596\.8,0\.0"[^>]*stroke="var\(--chart-critical\)"[^>]*stroke-width="12"/);
  assert.ok(extremeTemperature.includes('110.0°C')&&extremeTemperature.includes('95°C')&&extremeTemperature.includes('30°C'));
  assert.ok(extremeTemperature.includes('history-chart__temperature--critical')&&extremeTemperature.includes('温度 &gt;95°C'));
  assert.ok(!hotCurve.includes('history-chart__temperature-above-range'),'70–95°C remains the normal red width');
  const exactly95=renderToStaticMarkup(React.createElement(HistoryChart,{title:'GPU',headline:'fixture',detail:'fixture',series:[],maxValue:100,maxLabel:'100%',windowEnd:new Date(end).toISOString(),temperature:{value:95,detail:'fixture',gapSeconds:35,points:[{at,value:95}]}}));
  assert.ok(!exactly95.includes('history-chart__temperature-above-range'),'exactly 95°C does not trigger the extra thick line');
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
  const {NodeCard}=await server.ssrLoadModule('/src/components/NodeCard.tsx');
  const multiGpus=[0,1,2,3].map(index=>({...metrics.gpus[0],index,name:`Fixture GPU ${index}`,utilization_percent:index*20,
    memory_used_bytes:(index+1)*1024**3,memory_total_bytes:24*1024**3,temperature_celsius:45+index*12}));
  const multiNode={...props.node,id:'multi-fixture',hostname:'Multi GPU fixture',metrics:{...metrics,gpus:[multiGpus[2],multiGpus[0],multiGpus[3],multiGpus[1]]}};
  const cardProps={node:multiNode,liveSamples:[],temperatureSnapshots:[],checkedAt:end,onElevate:()=>{},elevationAvailable:false};
  {
    const {NodePanel}=await server.ssrLoadModule('/src/components/NodePanel.tsx');
    let bodyCalls=0;
    const panel=(node)=>renderToStaticMarkup(React.createElement(NodePanel,{node,
      header:React.createElement('header',null,React.createElement('h2',null,node.hostname),React.createElement('button',null,'恢复监控')),
      notice:React.createElement('p',{role:'alert'},'visible-control-error'),
      children:()=>{bodyCalls++;return React.createElement('p',null,'lazy-body-fixture');}}));
    const pausedNode={...multiNode,online:false,monitoring_paused:true,monitoring_revision:3,monitoring_applied_revision:2,
      metrics:{...multiNode.metrics,disks:[{name:'/dev/synthetic',mount_point:'/paused-storage',file_system:'ext4',total_bytes:1000,available_bytes:500}],block_devices:[]}};
    const pending=panel(pausedNode);
    const confirmed=panel({...pausedNode,monitoring_applied_revision:3});
    assert.equal(bodyCalls,0,'neither pending nor confirmed paused panels render their detailed body');
    for(const html of [pending,confirmed])assert.ok(html.includes('aria-expanded="false"')&&html.includes('aria-controls=')&&html.includes('恢复监控')&&html.includes('visible-control-error')&&html.includes('hidden=""')&&!html.includes('lazy-body-fixture'));
    const offline=panel({...pausedNode,monitoring_paused:false});
    assert.equal(bodyCalls,1);
    assert.ok(offline.includes('lazy-body-fixture')&&!offline.includes('node-panel--paused'));
    const pausedCard=renderToStaticMarkup(React.createElement(NodeCard,{...cardProps,node:pausedNode}));
    assert.ok(pausedCard.includes('暂停待确认')&&pausedCard.includes('设备详情')&&pausedCard.includes('历史使用记录')&&pausedCard.includes('aria-expanded="false"'));
    assert.ok(!pausedCard.includes('class="history-chart"')&&!pausedCard.includes('收起 GPU'));
    for(const name of ['Storage','Network','Users','PermissionChecks','Security','OSandSoftwares','AiTools','ProgrammingEnvironment','GlobalNetwork']) {
      const page=await server.ssrLoadModule(`/src/pages/${name}.tsx`);
      const html=renderToStaticMarkup(React.createElement(page[name],{nodes:[pausedNode],targets:[]}));
      assert.ok(html.includes('node-panel--paused')&&html.includes('aria-expanded="false"'),`${name} uses paused disclosure`);
      assert.ok(html.includes('暂停待确认'),`${name} keeps pause status visible`);
      const card=html.match(/<article class="[^"]*node-panel--paused[^>]*>([\s\S]*?)<\/article>/)?.[1];
      assert.ok(card&&!/class="(?:storage-device-card|history-chart|overlay-health-grid|permission-table)"/.test(card),`${name} omits collapsed detail rendering`);
    }
    console.log('PASS paused/pending machines default closed across all monitoring cards, retain management controls and skip detail rendering; ordinary offline machines stay expanded');
  }
  const defaultGpuCard=renderToStaticMarkup(React.createElement(NodeCard,cardProps));
  assert.ok(defaultGpuCard.includes('收起 GPU')&&defaultGpuCard.includes('aria-expanded="true"'));
  assert.ok(defaultGpuCard.includes('href="/usage-history?node=multi-fixture" target="_blank" rel="noopener noreferrer"'));
  assert.ok(!/class="node-gpu-details" hidden=""/.test(defaultGpuCard));
  assert.equal((defaultGpuCard.match(/class="history-chart"/g)??[]).length,9,'multiple GPUs expand by default');
  const collapsedDetails=renderToStaticMarkup(React.createElement(NodeCharts,{...props,node:multiNode,gpuExpanded:false}));
  assert.ok(/class="node-gpu-details" hidden=""/.test(collapsedDetails));
  assert.equal((collapsedDetails.match(/class="history-chart"/g)??[]).length,5,'explicitly collapsed details do not render extra charts');
  const singleCard=renderToStaticMarkup(React.createElement(NodeCard,{...cardProps,node:{...multiNode,metrics}}));
  assert.ok(!singleCard.includes('展开所有 GPU'));
  assert.ok(!singleCard.includes('展开所有 CPU'));
  assert.ok(!singleCard.includes('收起 GPU')&&!singleCard.includes('收起 CPU'));
  const sockets=[0,1].map(id=>({id,model:`Socket ${id}`,logical_cpus:96,physical_cores:48,usage_percent:20+id*50,frequency:{collected_at:at,average_mhz:2800,minimum_mhz:800,maximum_mhz:4200,logical_cpus:96},temperature_celsius:null}));
  const multiCpuNode={...multiNode,cpu_logical_cores:192,metrics:{...multiNode.metrics,cpu_sockets:sockets}};
  const multiCpuCard=renderToStaticMarkup(React.createElement(NodeCard,{...cardProps,node:multiCpuNode}));
  assert.ok(multiCpuCard.includes('收起 CPU')&&multiCpuCard.includes('收起 GPU'));
  assert.ok(!/class="node-cpu-details" hidden=""/.test(multiCpuCard));
  assert.ok(multiCpuCard.includes('Socket 0'),'multiple CPUs expand by default');
  assert.equal((multiCpuCard.match(/class="history-chart"/g)??[]).length,11);
  const collapsedCpus=renderToStaticMarkup(React.createElement(NodeCharts,{...props,node:multiCpuNode,cpuExpanded:false,gpuExpanded:false}));
  assert.ok(/class="node-cpu-details" hidden=""/.test(collapsedCpus));
  assert.equal((collapsedCpus.match(/class="history-chart"/g)??[]).length,5);
  const cpuExpanded=renderToStaticMarkup(React.createElement(NodeCharts,{...props,node:multiCpuNode,cpuExpanded:true,gpuExpanded:false}));
  assert.equal((cpuExpanded.match(/class="history-chart"/g)??[]).length,7);
  assert.ok(cpuExpanded.includes('CPU 0')&&cpuExpanded.includes('CPU 1')&&cpuExpanded.includes('Socket 0')&&cpuExpanded.includes('96 线程 / 48 核'));
  const bothExpanded=renderToStaticMarkup(React.createElement(NodeCharts,{...props,node:multiCpuNode,cpuExpanded:true,gpuExpanded:true}));
  assert.equal((bothExpanded.match(/class="history-chart"/g)??[]).length,11);
  const singleSocket=renderToStaticMarkup(React.createElement(NodeCard,{...cardProps,node:{...multiCpuNode,metrics:{...multiCpuNode.metrics,cpu_sockets:[sockets[0]]}}}));
  assert.ok(!singleSocket.includes('展开所有 CPU'),'one 96-thread CPU must not be mistaken for multiple sockets');
  assert.ok(!singleSocket.includes('收起 CPU'));
  console.log('PASS multi-CPU/GPU default expansion, independent explicit collapse, physical device counts and single-device omission');
  const expandedGpus=renderToStaticMarkup(React.createElement(NodeCharts,{...props,node:multiNode,gpuExpanded:true,gpuDetailsId:'fixture-expanded-gpus'}));
  const gpuDetails=expandedGpus.slice(expandedGpus.indexOf('id="fixture-expanded-gpus"'));
  assert.equal((expandedGpus.match(/class="history-chart"/g)??[]).length,9);
  assert.ok(gpuDetails.indexOf('GPU 0')<gpuDetails.indexOf('GPU 1')&&gpuDetails.indexOf('GPU 1')<gpuDetails.indexOf('GPU 2'));
  assert.ok(gpuDetails.includes('1.0/24.0 GiB')&&gpuDetails.includes('4.0/24.0 GiB')&&gpuDetails.includes('81.0°C'));
  assert.equal((gpuDetails.match(/stroke="var\(--chart-blue\)"/g)??[]).length,4);
  assert.equal((gpuDetails.match(/history-chart__temperature--hot/g)??[]).length,1);
  console.log('PASS multi-GPU disclosure, lazy per-device charts, individual VRAM/temperature readings and single-GPU compatibility');
  const {UsageHistoryCharts,UsageOverheatingNotice}=await server.ssrLoadModule('/src/pages/UsageHistory.tsx');
  const historyStat=value=>({average:value,maximum:value,minimum:value,samples:value===null?0:1});
  const gpuStat={core:historyStat(30),memory:historyStat(1024**3),memory_percent:historyStat(20),temperature:historyStat(60),thermal_throttling:{samples:5,detected_samples:1}};
  const usageStats={cpu:historyStat(25),memory:historyStat(4*1024**3),network_rx:historyStat(100),network_tx:historyStat(200),disk_read:historyStat(null),disk_write:historyStat(null),cpu_temperature:historyStat(55),ssd_temperature:historyStat(null),gpu_count:{...historyStat(4),maximum:8},gpu:gpuStat,gpus:{'7':gpuStat}};
  const usageStart=new Date(end-86400*7*1000).toISOString();
  const usageData={node_id:'fixture',hostname:'History fixture',start:usageStart,end:new Date(end).toISOString(),utc_offset_minutes:480,bucket_seconds:600,summary:usageStats,
    days:[{date:'2026-11-01',start:usageStart,end:new Date(end).toISOString(),summary:usageStats}],gpus:[{index:7,name:'Historic GPU 7'}],points:[{at,stats:usageStats}]};
  const usageHtml=renderToStaticMarkup(React.createElement(UsageHistoryCharts,{data:usageData,gpu:'7'}));
  assert.equal((usageHtml.match(/class="history-chart"/g)??[]).length,6);
  assert.ok(usageHtml.includes('GPU 7')&&usageHtml.includes('Historic GPU 7')&&usageHtml.includes('近 7 天')&&usageHtml.includes('4–8 张'));
  assert.ok(usageHtml.includes('本时段没有有效采样')&&!usageHtml.includes('10080 分钟'));
  assert.ok(usageHtml.includes('Overheating')&&usageHtml.includes('本时段硬件曾报告热降频')&&usageHtml.includes('GPU 7 · 1 个热降频采样'));
  const coolGpu={...gpuStat,temperature:historyStat(95),thermal_throttling:{samples:5,detected_samples:0}};
  const coolStats={...usageStats,cpu_temperature:historyStat(null),gpu:coolGpu,gpus:{'7':coolGpu}};
  const dayData={...usageData,days:[{...usageData.days[0],summary:coolStats}]};
  const coolDay=renderToStaticMarkup(React.createElement(UsageHistoryCharts,{data:dayData,period:'2026-11-01',gpu:'7'}));
  assert.ok(!coolDay.includes('Overheating'),'a non-throttled day must not inherit another day’s warning, even at 95°C');
  const dailyHeat=renderToStaticMarkup(React.createElement(UsageOverheatingNotice,{stats:usageStats,gpu:'7',compact:true}));
  assert.ok(dailyHeat.includes('Overheating')&&dailyHeat.includes('当日硬件曾报告热降频')&&dailyHeat.includes('GPU 7 · 1 个热降频采样'));
  assert.ok(renderToStaticMarkup(React.createElement(UsageOverheatingNotice,{stats:coolStats,compact:true})).includes('未记录热降频'));
  const legacyStats={...coolStats,gpu:{...coolGpu,thermal_throttling:undefined}};
  const legacyThermal=renderToStaticMarkup(React.createElement(UsageOverheatingNotice,{stats:legacyStats,compact:true}));
  assert.ok(legacyThermal.includes('未采集降频状态')&&!legacyThermal.includes('Overheating'));
  console.log('PASS historical overheating requires selected day/GPU hardware evidence; temperature alone and missing flags never trigger it');
  console.log('PASS standalone seven-day charts, GPU history selection, real gaps and long-window date labels');
  assert.equal((missing.match(/>温度 —<\/span>/g) ?? []).length, 3);
  assert.ok(!missing.includes(">0.0°C<"));
  console.log("PASS fixed 30–95°C axes, red above 70°C, extra thick only above 95°C at true crossings, unclipped readings and missing-sensor display");
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
  const wdStorage=renderToStaticMarkup(React.createElement(BlockDeviceMap,{devices:[
    {...devices[0],name:'wd-full',path:'/dev/sdb',model:'WDC WD40EFPX-68C6CN0'},
    {...devices[0],name:'wd-short',path:'/dev/fixture-wd',model:'WDC WD40EFPX-68C'},
  ],disks:[]}));
  assert.equal((wdStorage.match(/storage-recording-badge--cmr/g)??[]).length,2);
  assert.ok(!wdStorage.includes('storage-recording-badge--unknown')&&!wdStorage.includes('ⓘ'));
  assert.ok(wdStorage.includes('西数 WD Red Plus 官方规格表')&&wdStorage.includes('WD40EFPX-68C6CN0'));
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
  assert.equal((storagePage.match(/class="meter__fill meter__fill--storage"/g) ?? []).length, 7);
  const boot={...partition,name:'boot-fixture',path:'/dev/boot-fixture',partition_label:'EFI System Partition',mount_points:['/boot/efi']};
  const nfs={name:'10.0.0.1:/export',mount_point:'/nfs',file_system:'nfs4',total_bytes:0,available_bytes:0,read_bytes_per_second:null,written_bytes_per_second:null,
    mount:{device_id:'0:41',root:'/',source:'10.0.0.1:/export',network:true,read_only:false,capacity_known:false}};
  const mountPage=renderToStaticMarkup(React.createElement(Storage,{nodes:[{id:'mount-fixture',hostname:'mount-fixture',online:true,metrics:{...metrics,block_devices:[{...devices[0],children:[boot]}],disks:[mounted('/',99),mounted('/home/tzr/bigdata',99),{...mounted('/boot/efi',100),name:'/dev/boot-fixture'},nfs]}}]}));
  assert.ok(mountPage.includes('显示启动分区')&&mountPage.includes('aria-pressed="false"'));
  assert.ok(!mountPage.includes('boot-fixture')&&!mountPage.includes('/boot/efi'));
  assert.ok(mountPage.includes('不是空闲空间')&&mountPage.includes('共享容量 · 2 个挂载点'));
  assert.ok(mountPage.includes('/nfs')&&mountPage.includes('网络共享 · NFS4')&&mountPage.includes('容量未采集')&&mountPage.includes('未探测远端容量'));
  const cachedNfs={...nfs,total_bytes:1000,available_bytes:300,mount:{...nfs.mount,capacity_known:true,capacity_probe:{interval_seconds:3600,attempted_at:at,collected_at:'2020-01-01T00:00:00Z',status:'timeout'}}};
  const cachedMountPage=renderToStaticMarkup(React.createElement(Storage,{nodes:[{id:'cached-nfs',hostname:'cached-nfs',online:true,metrics:{...metrics,disks:[cachedNfs]}}]}));
  assert.ok(cachedMountPage.includes('每小时采集')&&cachedMountPage.includes('采集于')&&cachedMountPage.includes('查询超时')&&cachedMountPage.includes('保留上次结果')&&cachedMountPage.includes('结果已过期'));
  assert.ok(!cachedMountPage.includes('容量未采集')&&cachedMountPage.includes('700 B / 1000 B'));
  const shownBoot=renderToStaticMarkup(React.createElement(BlockDeviceMap,{devices:[{...devices[0],children:[boot]}],disks:[],showBoot:true}));
  assert.ok(shownBoot.includes('/boot/efi')&&shownBoot.includes('boot-fixture'));
  console.log('PASS boot partitions hidden by default with preserved geometry, shared filesystem labels and NFS unknown capacity');
  assert.equal((storagePage.match(/class="number storage-table__low-space"/g) ?? []).length, 2);
  assert.equal((storagePage.match(/class="meter__warning" role="img"/g) ?? []).length, 2);
  assert.equal((storagePage.match(/⚠/g) ?? []).length, 2);
  assert.ok(storagePage.includes('--meter-accent:var(--storage-green)'));
  assert.ok(storagePage.includes('--meter-accent:color-mix(in srgb, var(--storage-orange-light) 50.0%, var(--storage-orange-deep) 50.0%)'));
  assert.ok(storagePage.includes('--meter-accent:var(--chart-hot)'));
  const invalidRow = storagePage.match(/<tr><td><strong>\/data\/invalid<\/strong>[\s\S]*?<\/tr>/)?.[0];
  assert.ok(invalidRow && invalidRow.includes('容量未采集') && !invalidRow.includes('meter__warning'));
  console.log("PASS mounted filesystem usage: low green, 20–90 graded orange, >90 red with warning, invalid unknown, duplicate mount points consistent");
  const { StorageCell, SummaryCell, ResourceAccounting } = await server.ssrLoadModule("/src/pages/PlatformHealth.tsx");
  const resourceRow=renderToStaticMarkup(React.createElement(ResourceAccounting,{name:"Measured service",usage:{pid:7,cpu_percent_of_host:.001,cpu_percent_of_one_core:1.25,sample_interval_seconds:10.5,children_cpu_percent_of_one_core:.5,
    logical_read_bytes_per_second:1024,logical_written_bytes_per_second:512,
    service:{unit:"easymanage-server.service",cpu_percent_of_one_core:2.5,memory_current_bytes:8192,disk_read_bytes_per_second:0,disk_written_bytes_per_second:128}}}));
  assert.ok(resourceRow.includes("1.25%")&&resourceRow.includes("2.50%")&&resourceRow.includes("10.5 秒")&&resourceRow.includes("8 KiB")&&resourceRow.includes("已退出子进程"));
  const unscoped=renderToStaticMarkup(React.createElement(ResourceAccounting,{name:"SSH daemon",usage:{pid:8}}));
  assert.ok(unscoped.includes("无独立服务口径")&&!unscoped.includes("0.00%"));
  const offlineResource=renderToStaticMarkup(React.createElement(ResourceAccounting,{name:"Offline Agent",usage:{pid:8,cpu_percent_of_one_core:1.25},online:false,collectedAt:"2026-01-01T00:00:00Z"}));
  assert.ok(offlineResource.includes("节点离线 · 历史采样")&&offlineResource.includes("1.25%"));
  const pausedResource=renderToStaticMarkup(React.createElement(ResourceAccounting,{name:"Paused Agent",usage:{pid:8},paused:true}));
  assert.ok(pausedResource.includes("监控已暂停 · 历史采样"));
  const weighted=renderToStaticMarkup(React.createElement(SummaryCell,{metric:{samples:2,average:2,maximum:3,time_weighted_average:2.5,observed_seconds:40,total:10000},format:String}));
  assert.ok(weighted.includes("区间均值 2.5")&&weighted.includes("精确区间 40 秒")&&weighted.includes("区间累计"));
  const client=await server.ssrLoadModule("/src/api/client.ts");
  const originalFetch=globalThis.fetch;const seen=[];
  try {
    const replies=[new Response(JSON.stringify({nodes:[],generated_at:"fixture"}),{headers:{ETag:'W/"light"'}}),new Response(null,{status:304}),new Response(JSON.stringify({nodes:[],generated_at:"full"}),{headers:{ETag:'W/"full"'}})];
    globalThis.fetch=async(url,options)=>{seen.push({url,headers:options.headers});return replies.shift();};
    const first=await client.fetchNodes(undefined,false);const same=await client.fetchNodes(undefined,false);
    assert.equal(first,same,"304 must retain data identity and avoid a redundant React tree update");
    await client.fetchNodes(undefined,true);
    assert.equal(seen[1].headers['If-None-Match'],'W/"light"');
    assert.equal(seen[2].headers['If-None-Match'],undefined,"full and lightweight inventories need separate validators");
    assert.ok(seen[0].url.endsWith('include_software=false')&&seen[2].url.endsWith('include_software=true'));
  } finally {globalThis.fetch=originalFetch;}
  console.log("PASS resource scopes, real interval weighting, missing measurements and stable conditional fleet views");
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
  const { MachineManagement, AddMachineDialog, MachineMonitoringButton, machineMonitoringLabel, MachineSshDialog, RedeployMachineDialog, SshAuthenticationFields, AgentConnectionSettings, AgentDeploymentFields } = await server.ssrLoadModule("/src/components/MachineManagement.tsx");
  const pausedMachine={monitoring_paused:true,monitoring_revision:1,monitoring_applied_revision:1,monitoring_supported:true,control_seen_at:new Date().toISOString()};
  assert.equal(machineMonitoringLabel(pausedMachine),"已暂停 · 轻量心跳在线");
  assert.equal(machineMonitoringLabel({...pausedMachine,monitoring_supported:false}),"暂停待确认 · 需更新 Agent");
  assert.equal(machineMonitoringLabel({...pausedMachine,monitoring_applied_revision:0}),"暂停待确认");
  assert.equal(machineMonitoringLabel({...pausedMachine,control_seen_at:"2020-01-01T00:00:00Z"}),"已暂停 · 轻量心跳离线");
  assert.match(machineMonitoringLabel({...pausedMachine,monitoring_paused:false,monitoring_revision:2}),/恢复待确认/);
  assert.equal(machineMonitoringLabel({...pausedMachine,monitoring_paused:false}),"已恢复 · 等待新指标");
  const legacyMonitoring={monitoring_paused:false,monitoring_supported:false,monitoring_revision:2,monitoring_applied_revision:null,last_seen:new Date().toISOString()};
  assert.equal(machineMonitoringLabel(legacyMonitoring),"在线",'legacy resume uses real heartbeats rather than an acknowledgement it cannot send');
  assert.equal(machineMonitoringLabel({...legacyMonitoring,last_seen:'2020-01-01T00:00:00Z'}),"离线 / 等待部署");
  assert.equal(machineMonitoringLabel({...legacyMonitoring,monitoring_paused:true}),"暂停待确认 · 需更新 Agent");
  const monitoringButton=(machine,busy=false)=>renderToStaticMarkup(React.createElement(MachineMonitoringButton,{machine:{hostname:"Paused fixture",...machine},busy,onToggle:()=>{}}));
  assert.ok(monitoringButton(pausedMachine).includes('aria-label="Paused fixture 恢复监控"'));
  assert.ok(monitoringButton({...pausedMachine,monitoring_paused:false}).includes('aria-label="Paused fixture 暂停监控"'));
  assert.ok(monitoringButton(legacyMonitoring).includes('当前 Agent 需先更新'));
  assert.ok(monitoringButton({}).includes("需先更新平台后端")&&monitoringButton({}).includes('disabled=""'));
  assert.ok(monitoringButton(pausedMachine,true).includes('disabled=""'));
  const stoppedMachine={node_id:'stopped-fixture',hostname:'Stopped fixture',deployment_mode:'basic',agent_service_state:'stopped',agent_service_desired_stopped:true,agent_service_revision:1,agent_stop_available:true,...pausedMachine};
  assert.equal(machineMonitoringLabel(stoppedMachine),'Agent 已停止 · 需重新部署');
  assert.ok(monitoringButton(stoppedMachine).includes('disabled=""')&&monitoringButton(stoppedMachine).includes('只能重新部署'));
  const {AgentStopButton,AgentStopDialog}=await server.ssrLoadModule('/src/components/AgentStopDialog.tsx');
  const stopButton=machine=>renderToStaticMarkup(React.createElement(AgentStopButton,{machine,busy:false,onStop:()=>{}}));
  assert.ok(stopButton({...stoppedMachine,agent_service_state:null,agent_service_desired_stopped:false}).includes('>停止 Agent</button>'));
  assert.ok(!stopButton({...stoppedMachine,agent_service_state:null,agent_service_desired_stopped:false}).includes('disabled=""'));
  assert.ok(stopButton(stoppedMachine).includes('Agent 已停止')&&stopButton(stoppedMachine).includes('disabled=""'));
  const stopDialog=renderToStaticMarkup(React.createElement(AgentStopDialog,{machine:{...stoppedMachine,saved_credentials_usable:true},onClose:()=>{},onStopped:()=>{}}));
  assert.ok(stopDialog.includes('继续使用只能重新部署 Agent')&&stopDialog.includes('保留 Agent 文件'));
  assert.ok(stopDialog.includes('确认停止 Agent')&&stopDialog.includes('使用平台已加密保存的 SSH 凭据')&&!stopDialog.includes('type="password"'));
  const stoppedRedeploy=renderToStaticMarkup(React.createElement(RedeployMachineDialog,{machine:{...stoppedMachine,deployment_mode:'full'},onClose:()=>{},onStarted:()=>{}}));
  assert.ok(stoppedRedeploy.includes('管理员鉴权')&&stoppedRedeploy.includes('重新部署会恢复 Agent 和监控'));
  console.log("PASS monitoring pause distinguishes confirmed, pending, unsupported, disconnected and resume states");
  const { NodeStatus }=await server.ssrLoadModule("/src/components/NodeStatus.tsx");
  const pausedStatus=(node)=>renderToStaticMarkup(React.createElement(NodeStatus,{node}));
  assert.ok(pausedStatus({monitoring_paused:true,monitoring_revision:1,monitoring_applied_revision:0,online:true}).includes("暂停待确认"));
  const confirmedStatus=pausedStatus({monitoring_paused:true,monitoring_revision:1,monitoring_applied_revision:1,online:false});
  assert.ok(confirmedStatus.includes("监控已暂停")&&confirmedStatus.includes("status--paused")&&!confirmedStatus.includes("status--offline"));
  assert.ok(pausedStatus({...stoppedMachine,online:false}).includes('Agent 已停止')&&!pausedStatus({...stoppedMachine,online:false}).includes('监控已暂停'));
  const management=renderToStaticMarkup(React.createElement(MachineManagement,{onRefresh:()=>{}}));
  assert.ok(management.includes("机器管理") && management.includes("添加机器") && management.includes("软删除保留历史和身份"));
  assert.ok(management.includes("查看受管机器与已删除机器") && management.includes("外部平台入口管理"));
  assert.ok(management.includes('<details class="machine-list" open="">'),'machine action buttons are in the default-open management list');
  assert.ok(!management.includes('aria-label="Agent 监控开关"'),'monitoring uses the shared action buttons without a separate module');
  assert.ok(!/<button[^>]*disabled[^>]*>添加机器<\/button>/.test(management),'external shortcuts stay available without Agent candidates');
  const externalAdd=renderToStaticMarkup(React.createElement(AddMachineDialog,{agentAvailable:false,onClose:()=>{},onStarted:()=>{},onExternalSaved:()=>{}}));
  assert.ok(externalAdd.includes('value="external" selected=""')&&externalAdd.includes('type="url"')&&externalAdd.includes('value="agent" disabled=""'));
  assert.ok(!externalAdd.includes('type="password"')&&!externalAdd.includes('type="file"'));
  const agentAdd=renderToStaticMarkup(React.createElement(AddMachineDialog,{agentAvailable:true,onClose:()=>{},onStarted:()=>{},onExternalSaved:()=>{}}));
  assert.ok(agentAdd.includes('value="agent" selected=""')&&agentAdd.includes('value="basic" selected=""')&&agentAdd.includes('检查 SSH 主机指纹'));
  const {ExternalPlatformCard,ExternalPlatformManagement}=await server.ssrLoadModule('/src/components/ExternalPlatform.tsx');
  const externalFixture={id:'external-fixture',name:'External <script>fixture</script>',url:'https://compute.example/gpu#console',note:'GPU cloud entry',revision:1,created_at:'fixture',updated_at:'fixture',deleted_at:null};
  const externalCard=renderToStaticMarkup(React.createElement(ExternalPlatformCard,{platform:externalFixture,onChanged:()=>{}}));
  assert.ok(externalCard.includes('前往算力平台')&&externalCard.includes('external-platform-card__launch')&&externalCard.includes('href="https://compute.example/gpu#console" target="_blank" rel="noopener noreferrer"'));
  assert.ok(!externalCard.includes('<script>fixture</script>')&&!externalCard.includes('history-chart')&&!externalCard.includes('status--online'));
  const invalidExternal=renderToStaticMarkup(React.createElement(ExternalPlatformCard,{platform:{...externalFixture,url:'javascript:alert(1)'},onChanged:()=>{}}));
  assert.ok(!invalidExternal.includes('href="javascript:')&&invalidExternal.includes('入口链接无效'));
  const archivedExternal=renderToStaticMarkup(React.createElement(ExternalPlatformManagement,{platforms:[{...externalFixture,deleted_at:'fixture'}],onChanged:()=>{}}));
  assert.ok(archivedExternal.includes('已软删除')&&archivedExternal.includes('>恢复</button>'));
  console.log('PASS external compute cards have a prominent safe link, no fake telemetry, independent add mode and recoverable soft deletion');
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
  assert.ok(redeployDialog.includes("覆盖修复")&&redeployDialog.includes("保留历史和本地数据库")&&redeployDialog.includes("其他机器 ID")&&redeployDialog.includes("disabled"));
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
