import type { BlockDevice } from "../types/monitoring.ts";

export interface RecordingEvidence {
  kind: "smr" | "cmr" | "unknown";
  label: string;
  matchedModel: string | null;
  explanation: string;
  sources: { title: string; url: string }[];
  verifiedAt: string | null;
}

// Explicit manufacturer-documented models only. Never infer from family, capacity,
// RPM/cache, sequential-write speed or Linux queue/zoned == "none".
const BARRACUDA = {
  title: "Seagate BarraCuda DS1900.14 · 第 2 页 Recording Technology",
  url: "https://www.seagate.com/www-content/datasheets/pdfs/3-5-barracudaDS1900-14-2007US-en_US.pdf",
};
const EXOS = {
  title: "Seagate Exos 7E8 DS1957.1 · 第 1 页 CMR，第 2 页型号表",
  url: "https://www.seagate.com/content/dam/seagate/migrated-assets/www-content/datasheets/pdfs/exos-7-e8-data-sheet-DS1957-1-1709US-en_US.pdf",
};
const MODELS: Readonly<Record<string, "smr" | "cmr">> = {
  ST2000DM005: "smr",
  ST4000DM004: "smr",
  ST4000NM0035: "cmr",
};

export function diskRecording(device: Pick<BlockDevice, "kind" | "model" | "rotational">): RecordingEvidence | null {
  // Classify disks, not their partitions, RAID/LVM volumes or loop devices.
  if (device.kind !== "disk") return null;
  const model = device.model?.trim().toUpperCase() ?? "";
  // Accept the device suffix reported by lsblk and an optional manufacturer name,
  // but do not allow substring matches (e.g. ST2000DM0050 or arbitrary bridge text).
  const normalized = /^(?:SEAGATE\s+)?(ST\d+[A-Z]{2}\d+)(?:-[A-Z0-9]+)?$/.exec(model)?.[1];
  const kind = normalized ? MODELS[normalized] : undefined;
  if (kind) return {
    kind,
    label: kind === "smr" ? "SMR 叠瓦盘" : "CMR 垂直盘",
    matchedModel: normalized!,
    explanation: kind === "smr"
      ? "厂商规格表对该型号明确标注 SMR（叠瓦式磁记录）；此标签不是故障告警，不代表已测得当前写入性能。"
      : "厂商 Exos 7E8 资料明确说明采用 CMR（传统磁记录），且型号表列出该型号；并非根据转速或缓存推断。",
    sources: [kind === "smr" ? BARRACUDA : EXOS],
    verifiedAt: "2026-09-29",
  };
  // Existing inventory identifies non-rotational devices separately; SSDs are not CMR.
  if (device.rotational === false) return null;
  return {
    kind: "unknown", label: "记录方式未确认", matchedModel: null,
    explanation: model
      ? "该型号尚未录入有厂商依据的型号表，不能仅凭容量、转速或同系列名称判定 SMR / CMR。"
      : "Agent 尚未提供可识别的硬盘型号，无法核对厂商的 SMR / CMR 规格。",
    sources: [], verifiedAt: null,
  };
}
