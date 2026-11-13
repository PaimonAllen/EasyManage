import {isPartition,isWindowsDevice,deviceMountPoints,blockDeviceTitle,solidState,windowsMountKey} from "../lib/storageTopology";
import { useId, useState } from "react";
import { formatBytes } from "../lib/format";
import { diskRecording } from "../lib/diskRecording";
import { capacityKnown, isBootPartition } from "../lib/storageMounts";
import type { BlockDevice, DiskMetric } from "../types/monitoring";

const PARTITION_COLORS = ["#4de2c5", "#9b86f9", "#f3ba63", "#6ab8f2", "#e68cb4", "#93cb78"];

type Segment = { device: BlockDevice; start: number; size: number };

function mountedUsage(device: BlockDevice, disks: DiskMetric[]) {
  const path = device.path ?? `/dev/${device.name}`;
  const mounts=deviceMountPoints(device);
  const disk = disks.find((item) => isWindowsDevice(device)
    ? mounts.some(mount=>windowsMountKey(mount)===windowsMountKey(item.mount_point))
    : item.name === path && mounts.includes(item.mount_point));
  if (!disk || !capacityKnown(disk) || !Number.isFinite(disk.total_bytes) || !Number.isFinite(disk.available_bytes)
    || disk.total_bytes <= 0 || device.size_bytes <= 0 || disk.available_bytes < 0
    || disk.available_bytes > disk.total_bytes) return null;
  const usedBytes = disk.total_bytes - disk.available_bytes;
  return { usedBytes, totalBytes: disk.total_bytes, widthPercent: Math.min(100, usedBytes / device.size_bytes * 100) };
}

function layout(device: BlockDevice): { segments: Segment[]; positioned: boolean; remainder: number } {
  const partitions = device.children.filter((child) => isPartition(child) && child.size_bytes > 0);
  const total = device.size_bytes;
  if (total <= 0 || partitions.length === 0) return { segments: [], positioned: false, remainder: 0 };
  const sorted = [...partitions].sort((left, right) => (left.start_bytes ?? 0) - (right.start_bytes ?? 0));
  let end = 0;
  const positioned = sorted.every((part) => {
    const start = part.start_bytes;
    if (start == null || !Number.isFinite(start) || start < end || start + part.size_bytes > total) return false;
    end = start + part.size_bytes;
    return true;
  });
  if (positioned) return {
    segments: sorted.map((part) => ({ device: part, start: part.start_bytes!, size: part.size_bytes })),
    positioned: true,
    remainder: Math.max(0, total - sorted.reduce((sum, part) => sum + part.size_bytes, 0)),
  };
  // Unknown geometry: show relative partition sizes but do not label the unused width as free.
  const denominator = Math.max(total, partitions.reduce((sum, part) => sum + part.size_bytes, 0));
  let cursor = 0;
  return {
    segments: partitions.map((part) => {
      const segment = { device: part, start: cursor, size: part.size_bytes };
      cursor += part.size_bytes;
      return segment;
    }).map((segment) => ({ ...segment, start: segment.start / denominator, size: segment.size / denominator })),
    positioned: false,
    remainder: 0,
  };
}

function DeviceRow({ device, depth, showBoot }: { device: BlockDevice; depth: number; showBoot:boolean }) {
  if(!showBoot&&isBootPartition(device))return null;
  const title = isWindowsDevice(device)?blockDeviceTitle(device):device.partition_label || device.label || device.name;
  const mounts=deviceMountPoints(device);
  const filesystem = device.filesystem?.trim() || null;
  return <>
    <div className="block-device-row" style={{ paddingLeft: `${Math.min(depth, 7) * 16 + 10}px` }}>
      <div className="block-device-row__identity">
        <span className="block-device-row__type">{device.kind}</span>
        <strong title={device.path ?? device.name}>{title}</strong>
        {(filesystem || isPartition(device)) && <span className={`block-device-row__filesystem ${filesystem ? "" : "block-device-row__filesystem--unknown"}`} title={`文件系统格式：${filesystem ?? "未识别"}`}>{filesystem ?? "未识别"}</span>}
        <code>{device.path ?? device.name}</code>
        {mounts.length>1&&<span className="storage-mount-badge">共享挂载 · {mounts.length} 个入口</span>}
      </div>
      <div className="block-device-row__details">
        <span>{formatBytes(device.size_bytes)}</span>
        {mounts.length > 0
          ? <span title={mounts.join(" · ")}>{mounts.join(" · ")}</span>
          : device.children.length > 0
            ? <span className="muted">{device.children.length} 个下属设备</span>
            : <span className="muted">未挂载</span>}
        {device.read_only && <span className="storage-readonly">只读设备</span>}
      </div>
    </div>
    {device.children.map((child, index) => <DeviceRow key={`${child.path ?? child.name}-${index}`} device={child} depth={depth + 1} showBoot={showBoot} />)}
  </>;
}

function DeviceCard({ device, disks, showBoot }: { device: BlockDevice; disks: DiskMetric[]; showBoot:boolean }) {
  const [showRecording, setShowRecording] = useState(false);
  const recordingId = useId();
  const recording = diskRecording(device);
  const { segments, positioned, remainder } = layout(device);
  const denominator = positioned ? device.size_bytes : 1;
  const lastSegment = segments.at(-1);
  const trailingFreeStart = positioned && lastSegment ? lastSegment.start + lastSegment.size : null;
  const trailingFreePercent = trailingFreeStart !== null && trailingFreeStart < device.size_bytes
    ? ((device.size_bytes - trailingFreeStart) / device.size_bytes) * 100 : 0;
  const knownHdd = recording?.kind === "smr" || recording?.kind === "cmr";
  const driveKind = knownHdd ? "机械硬盘" : solidState(device)?"固态硬盘":device.rotational===true?"机械硬盘":"块设备";
  const title=blockDeviceTitle(device);
  const childKind=isWindowsDevice(device)&&device.children.length>0&&device.children.every(isPartition)?"分区":"下属设备";
  return <section className="storage-device">
    <header className="storage-device__header">
      <div>
        <h3 title={device.path??device.name}>{title} <span>{device.kind === "disk" ? driveKind : device.kind}</span>{recording && <button
          type="button"
          className={`storage-recording-badge storage-recording-badge--${recording.kind}`}
          onClick={() => setShowRecording(value => !value)}
          aria-expanded={showRecording}
          aria-controls={recordingId}
          aria-label={`查看磁记录方式依据：${recording.label}`}
          title={`${recording.explanation} 点击查看来源`}
        >{recording.label}{recording.kind !== "cmr" && <> <span aria-hidden="true">ⓘ</span></>}</button>}</h3>
        <p>{[device.model, device.transport?.toUpperCase(), device.partition_table?.toUpperCase(), device.removable ? "可移除" : null, device.read_only ? "只读" : null].filter(Boolean).join(" · ") || "未提供设备型号或分区表"}</p>
      </div>
      <strong>{formatBytes(device.size_bytes)}</strong>
    </header>
    {recording && showRecording && <div className="storage-recording-evidence" id={recordingId}>
      <strong>{recording.label}</strong>
      <p>{recording.explanation}</p>
      <p>Agent 上报型号：<code>{device.model?.trim() || "未提供"}</code>{recording.matchedModel && <> · 匹配型号：<code>{recording.matchedModel}</code></>}{recording.verifiedAt && <> · 资料核对：{recording.verifiedAt}</>}</p>
      {recording.sources.map(source => <a key={source.url} href={source.url} target="_blank" rel="noopener noreferrer">{source.title} ↗</a>)}
      <p>仅按厂商型号资料标注，不运行磁盘性能测试、SMART 自检或任何写盘操作。</p>
    </div>}
    {segments.length > 0 ? <>
      <div className={`partition-map ${positioned ? "" : "partition-map--relative"}`} role="img" aria-label={`${title} 的分区容量图`}>
        {segments.map((segment, index) => {
          const usage = mountedUsage(segment.device, disks);
          const hiddenBoot=!showBoot&&isBootPartition(segment.device);
          return <span
            className={`partition-map__segment${hiddenBoot?" partition-map__segment--hidden-boot":""}`}
            key={`${segment.device.name}-${index}`}
            style={{ left: `${(segment.start / denominator) * 100}%`, width: `${(segment.size / denominator) * 100}%`, background: hiddenBoot?"var(--line)":PARTITION_COLORS[index % PARTITION_COLORS.length], color: hiddenBoot?"var(--line)":PARTITION_COLORS[index % PARTITION_COLORS.length] }}
            title={hiddenBoot?"启动分区已隐藏，保留实际位置；不是空闲空间":`${segment.device.path ?? segment.device.name} · ${formatBytes(segment.device.size_bytes)}${deviceMountPoints(segment.device).length ? ` · ${deviceMountPoints(segment.device).join(" · ")}` : " · 未挂载"}${usage ? ` · 已用 ${formatBytes(usage.usedBytes)} / 文件系统 ${formatBytes(usage.totalBytes)}` : " · 使用量未知"}`}
          >{!hiddenBoot&&usage && usage.widthPercent > 0 && <span className="partition-map__used" style={{ width: `${usage.widthPercent}%` }} />}
            {!hiddenBoot&&usage && usage.widthPercent > 0 && usage.widthPercent < 100 && <span className="partition-map__usage-boundary" style={{ left: `${usage.widthPercent}%` }} aria-hidden="true" />}
          </span>;
        })}
        {trailingFreePercent > 0 && <span className="partition-map__trailing-free" style={{ width: `${trailingFreePercent}%` }} aria-hidden="true" />}
      </div>
      <div className="partition-map__legend">
        {segments.map((segment, index) => !showBoot&&isBootPartition(segment.device)?null:<span key={`${segment.device.name}-${index}`}><i style={{ background: PARTITION_COLORS[index % PARTITION_COLORS.length] }} />{segment.device.name} · {formatBytes(segment.device.size_bytes)}</span>)}
        {!showBoot&&segments.some(segment=>isBootPartition(segment.device))&&<span><i style={{background:"var(--line)"}}/>启动分区已隐藏</span>}
        {positioned && remainder > 0 && <span><i className="partition-map__free" />未分配 / 保留 · {formatBytes(remainder)}</span>}
      </div>
      {!positioned && <p className="storage-device__note">分区起点不可读取，图中仅按容量排列，不表示真实磁盘位置。</p>}
    </> : <p className="storage-device__note">未识别到直接分区；可能整盘使用或存在未识别布局，不能据此判断空间可用。</p>}
    <details className="storage-device-details">
      <summary>设备与分区详情{device.children.length > 0 && <span>· {device.children.length} 个{childKind}</span>}</summary>
      <div className="block-device-tree"><DeviceRow device={device} depth={0} showBoot={showBoot}/></div>
    </details>
  </section>;
}

export function BlockDeviceMap({ devices, disks, showBoot=true }: { devices: BlockDevice[]; disks: DiskMetric[]; showBoot?:boolean }) {
  return <div className="storage-device-list">
    {devices.map((device, index) => <DeviceCard key={`${device.path ?? device.name}-${index}`} device={device} disks={disks} showBoot={showBoot}/>)}
  </div>;
}
