import { useRef, useState } from "react";
import { Meter } from "../components/Meter";
import { NodePanel } from "../components/NodePanel";
import { BlockDeviceMap } from "../components/BlockDeviceMap";
import { DiskSpaceExplorer } from "../components/DiskSpaceExplorer";
import {
  CollectionNotes,
  NodeStatus,
  SnapshotAge,
} from "../components/NodeStatus";
import { PageHeading } from "../components/PageHeading";
import { formatBytes, formatRate } from "../lib/format";
import { setAllPartitionDetails } from "../lib/storageDisclosure";
import { storageUsageOver90, storageUsagePercent } from "../lib/storageUsage";
import {capacityKnown,isBootMount,isBootPartition,isNetworkStorage,sharedStorage,storageMountKey,type SharedStorage} from "../lib/storageMounts";
import type { BlockDevice, DiskMetric, NodeSummary } from "../types/monitoring";

function partitions(devices: BlockDevice[]): BlockDevice[] {
  return devices.flatMap((device) => [
    ...(device.kind === "part" ? [device] : []),
    ...partitions(device.children),
  ]);
}

function StorageTopology({ devices, disks, showBoot }: { devices: BlockDevice[]; disks: DiskMetric[]; showBoot:boolean }) {
  const map = useRef<HTMLDivElement>(null);
  function setAll(expanded: boolean) {
    setAllPartitionDetails(map.current?.querySelectorAll<HTMLDetailsElement>(".storage-device-details") ?? [], expanded);
  }
  return <>
    <div className="storage-topology-toolbar">
      <h3 className="section-title">磁盘与分区拓扑</h3>
      {devices.length > 0 && <div className="storage-topology-toolbar__actions">
        <button className="button button--secondary button--small" type="button" onClick={() => setAll(true)}>全部展开</button>
        <button className="button button--secondary button--small" type="button" onClick={() => setAll(false)}>全部折叠</button>
      </div>}
    </div>
    <div ref={map}>
      {devices.length > 0
        ? <BlockDeviceMap devices={devices} disks={disks} showBoot={showBoot}/>
        : <p className="table-empty">暂未收到块设备清单；仍可查看下方已挂载文件系统。</p>}
    </div>
  </>;
}

export function Storage({ nodes }: { nodes: NodeSummary[] }) {
  const [showBoot,setShowBoot]=useState(false);
  return (
    <>
      <PageHeading
        eyebrow="Storage management · read only"
        title="存储状态与磁盘管理"
        description="物理盘、分区、文件系统和挂载关系的只读可视化 · 不执行分区、格式化或挂载操作"
      />
      <section className="page-content">
        <div className="storage-visibility-toolbar"><button type="button" className="button button--secondary button--small" aria-pressed={showBoot} onClick={()=>setShowBoot(value=>!value)}>{showBoot?"隐藏启动分区":"显示启动分区"}</button><span className="hint">启动分区默认隐藏；图中保留实际位置，不作为空闲空间。根分区 / 始终显示。</span></div>
        <p className="hint">分区图按可读取的实际起点和容量绘制；分区内斜线表示已挂载文件系统的已用空间，未挂载或无法匹配的分区不画斜线，但不代表空盘。空白只表示未被已识别分区覆盖的区域，可能包含保留空间，不能直接作为可分配空间使用。全部信息来自只读采集，页面不提供写盘操作。</p>
        <p className="hint">机械盘的 SMR / CMR 标签按已核对的厂商型号资料匹配，点击可查看依据；未收录型号显示“记录方式未确认”，固态硬盘不适用此分类。SMR 标签不表示磁盘故障。</p>
        {nodes.map((node) => {
          const devices = node.metrics?.block_devices ?? [];
          const physical = devices.filter((device) => device.kind === "disk");
          const allPartitions = partitions(devices);
          const unmounted = allPartitions.filter((device) => device.mount_points.length === 0).length;
          const disks=node.metrics?.disks??[];
          const bootDevices=new Set(allPartitions.filter(isBootPartition).map(d=>d.path??`/dev/${d.name}`));
          const visibleDisks=disks.filter(d=>d.mount_point==="/"||showBoot||(!isBootMount(d.mount_point)&&!bootDevices.has(d.name)));
          const sharing=sharedStorage(disks);
          return (
          <NodePanel node={node} key={node.id} header={
            <header className="panel-header">
              <div>
                <h2>
                  {node.hostname} <NodeStatus node={node} />
                </h2>
                <p>
                  {physical.length} 块物理盘 · {allPartitions.length} 个分区 · {unmounted} 个未挂载分区 · {node.metrics?.disks.length ?? 0} 个已上报挂载点
                  {" "}<span className="storage-snapshot-ages">
                    <SnapshotAge node={node} disk />
                    <SnapshotAge node={node} inventory />
                  </span>
                </p>
              </div>
              <div className="compact-stat">
                <span>Swap 使用</span>
                <strong>
                  {node.metrics
                    ? `${formatBytes(node.metrics.swap_used_bytes)} / ${formatBytes(node.metrics.swap_total_bytes)}`
                    : "—"}
                </strong>
              </div>
            </header>
          }>{() => <>
            <StorageTopology devices={devices} disks={disks} showBoot={showBoot}/>
            <details className="storage-mounted-section" open>
              <summary className="storage-mounted-toggle">
                <h3 className="section-title">已挂载文件系统与 I/O</h3>
              </summary>
              <div className="table-scroll">
              <table className="storage-table">
                <thead>
                  <tr>
                    <th>设备 / 挂载点</th>
                    <th>文件系统</th>
                    <th>空间使用</th>
                    <th>可用空间</th>
                    <th>读取 / 写入</th>
                  </tr>
                </thead>
                <tbody>
                  {visibleDisks.map((disk) => {
                    const known=capacityKnown(disk);
                    const shared=sharing.get(storageMountKey(disk));
                    const usedBytes = disk.total_bytes - disk.available_bytes;
                    const usage = storageUsagePercent(disk.total_bytes, disk.available_bytes);
                    const over90 = known && storageUsageOver90(disk.total_bytes, disk.available_bytes);
                    return (
                    <tr key={`${disk.name}-${disk.mount_point}`}>
                      <td>
                        <strong>{disk.mount_point}</strong>
                        <small>{disk.name}</small>
                        {isNetworkStorage(disk)&&<span className="storage-mount-badge">网络共享 · {disk.file_system.toUpperCase()}</span>}
                        {disk.file_system==="autofs"&&<span className="storage-mount-badge">自动挂载入口</span>}
                        {shared&&<SharedMountDetails disk={disk} group={shared}/>}
                        {disk.mount?.read_only&&<small>只读挂载</small>}
                      </td>
                      <td>
                        <code>{disk.file_system}</code>
                      </td>
                      <td>
                        {known?<Meter
                          label="容量"
                          value={usage}
                          detail={`${formatBytes(usedBytes)} / ${formatBytes(disk.total_bytes)}`}
                          tone="storage"
                          warning={over90 ? "存储使用率超过 90%，请检查并规划清理或扩容" : undefined}
                        />:<span className="hint">容量未采集{!disk.mount?.capacity_probe&&<small>{isNetworkStorage(disk)?"仅读取挂载信息，未探测远端容量":disk.file_system==="autofs"?"未主动触发自动挂载":"当前容量不可确认"}</small>}</span>}
                        <NetworkCapacityNote disk={disk}/>
                      </td>
                      <td className={`number${over90 ? " storage-table__low-space" : ""}`}>
                        {known?formatBytes(disk.available_bytes):"—"}
                      </td>
                      <td>
                        <span>
                          ↓{" "}
                          {disk.read_bytes_per_second == null
                            ? "未采集"
                            : formatRate(disk.read_bytes_per_second)}
                        </span>
                        <small>
                          ↑{" "}
                          {disk.written_bytes_per_second == null
                            ? "未采集"
                            : formatRate(disk.written_bytes_per_second)}
                        </small>
                      </td>
                    </tr>
                    );
                  })}
                </tbody>
              </table>
              {!visibleDisks.length && (
                <p className="table-empty">{disks.length?"当前挂载点均为已隐藏的启动分区。":"暂未收到存储指标。"}</p>
              )}
              </div>
              <CollectionNotes node={node} components={["disk", "storage"]} />
            </details>
          </>}</NodePanel>
          );
        })}
        <DiskSpaceExplorer nodes={nodes} showBoot={showBoot}/>
      </section>
    </>
  );
}

function NetworkCapacityNote({disk}:{disk:DiskMetric}) {
  const probe=disk.mount?.capacity_probe;
  if(!probe)return null;
  const labels:Record<string,string>={pending:"等待首次容量采集",collecting:"正在刷新容量",timeout:"查询超时",error:"查询失败",busy:"前次查询仍在结束，已跳过",changed:"挂载已变化，下次重试",interrupted:"前次查询被中断"};
  const failed=!["ok","pending","collecting"].includes(probe.status);
  const stale=!!probe.collected_at&&Date.now()-Date.parse(probe.collected_at)>probe.interval_seconds*2000;
  return <small className={failed||stale?"storage-capacity-warning":"hint"}>
    每小时采集{probe.collected_at?<> · 采集于 {new Date(probe.collected_at).toLocaleString()}</>:null}
    {probe.status!=="ok"&&<> · {labels[probe.status]??"容量暂不可确认"}</>}
    {probe.collected_at&&(failed||probe.status==="collecting")&&" · 保留上次结果"}
    {stale&&" · 结果已过期"}
  </small>;
}

function SharedMountDetails({disk,group}:{disk:DiskMetric;group:SharedStorage}){
  return <details className="storage-shared-mount"><summary><span className="storage-mount-badge">共享容量 · {group.members.length} 个挂载点</span></summary>
    <div><p>这些挂载点共用同一文件系统的容量，不能逐行相加；它们可能对应不同子目录。</p>
      {disk.mount&&<p>当前文件系统内路径：<code>{disk.mount.root}</code>{disk.mount.root!=="/"?" · 子目录挂载":""}</p>}
      <p>{group.precise?"按设备标识确认":"按已上报的设备名匹配"} · 参考入口：<code>{group.primary.mount_point}</code></p>
      <ul>{group.members.map(d=><li key={storageMountKey(d)}><code>{d.mount_point}</code>{d.mount?.root&&d.mount.root!=="/"&&<> → <code>{d.mount.root}</code></>}</li>)}</ul>
    </div></details>;
}
