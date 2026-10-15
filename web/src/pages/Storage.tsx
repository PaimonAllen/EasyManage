import { useRef } from "react";
import { Meter } from "../components/Meter";
import { BlockDeviceMap } from "../components/BlockDeviceMap";
import {
  CollectionNotes,
  NodeStatus,
  SnapshotAge,
} from "../components/NodeStatus";
import { PageHeading } from "../components/PageHeading";
import { formatBytes, formatRate } from "../lib/format";
import { setAllPartitionDetails } from "../lib/storageDisclosure";
import { storageUsageOver90, storageUsagePercent } from "../lib/storageUsage";
import type { BlockDevice, DiskMetric, NodeSummary } from "../types/monitoring";

function partitions(devices: BlockDevice[]): BlockDevice[] {
  return devices.flatMap((device) => [
    ...(device.kind === "part" ? [device] : []),
    ...partitions(device.children),
  ]);
}

function StorageTopology({ devices, disks }: { devices: BlockDevice[]; disks: DiskMetric[] }) {
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
        ? <BlockDeviceMap devices={devices} disks={disks} />
        : <p className="table-empty">暂未收到块设备清单；仍可查看下方已挂载文件系统。</p>}
    </div>
  </>;
}

export function Storage({ nodes }: { nodes: NodeSummary[] }) {
  return (
    <>
      <PageHeading
        eyebrow="Storage management · read only"
        title="存储状态与磁盘管理"
        description="物理盘、分区、文件系统和挂载关系的只读可视化 · 不执行分区、格式化或挂载操作"
      />
      <section className="page-content">
        <p className="hint">分区图按可读取的实际起点和容量绘制；分区内斜线表示已挂载文件系统的已用空间，未挂载或无法匹配的分区不画斜线，但不代表空盘。空白只表示未被已识别分区覆盖的区域，可能包含保留空间，不能直接作为可分配空间使用。全部信息来自只读采集，页面不提供写盘操作。</p>
        <p className="hint">机械盘的 SMR / CMR 标签按已核对的厂商型号资料匹配，点击可查看依据；未收录型号显示“记录方式未确认”，固态硬盘不适用此分类。SMR 标签不表示磁盘故障。</p>
        {nodes.map((node) => {
          const devices = node.metrics?.block_devices ?? [];
          const physical = devices.filter((device) => device.kind === "disk");
          const allPartitions = partitions(devices);
          const unmounted = allPartitions.filter((device) => device.mount_points.length === 0).length;
          return (
          <article className="data-panel" key={node.id}>
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
            <StorageTopology devices={devices} disks={node.metrics?.disks ?? []} />
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
                  {node.metrics?.disks.map((disk) => {
                    const usedBytes = disk.total_bytes - disk.available_bytes;
                    const usage = storageUsagePercent(disk.total_bytes, disk.available_bytes);
                    const over90 = storageUsageOver90(disk.total_bytes, disk.available_bytes);
                    return (
                    <tr key={`${disk.name}-${disk.mount_point}`}>
                      <td>
                        <strong>{disk.mount_point}</strong>
                        <small>{disk.name}</small>
                      </td>
                      <td>
                        <code>{disk.file_system}</code>
                      </td>
                      <td>
                        <Meter
                          label="容量"
                          value={usage}
                          detail={`${formatBytes(usedBytes)} / ${formatBytes(disk.total_bytes)}`}
                          tone="storage"
                          warning={over90 ? "存储使用率超过 90%，请检查并规划清理或扩容" : undefined}
                        />
                      </td>
                      <td className={`number${over90 ? " storage-table__low-space" : ""}`}>
                        {formatBytes(disk.available_bytes)}
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
              {!node.metrics?.disks.length && (
                <p className="table-empty">暂未收到存储指标。</p>
              )}
              </div>
              <CollectionNotes node={node} components={["disk", "storage"]} />
            </details>
          </article>
          );
        })}
      </section>
    </>
  );
}
