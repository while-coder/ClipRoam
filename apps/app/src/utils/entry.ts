const MANUAL_UPLOAD_LIMIT = 100 * 1024 * 1024;
import { formatFileSize } from "./format";
import type {
  ClipboardEntry,
  Device,
  LocalClipboardEntry,
} from "../types";

/**
 * Pure display helpers shared by the history and pending-upload views. Anything
 * that reads reactive state takes it as an argument so both views can feed it
 * from their own props.
 */

export function deviceName(devicesById: Record<string, Device>, entry: ClipboardEntry): string {
  return devicesById[entry.sourceDeviceId]?.name ?? "";
}

export function isHashing(entry: LocalClipboardEntry): boolean {
  return entry.summary.hashedCount < entry.summary.fileCount;
}

/**
 * Upload state is derived from the entry's summary: `storedCount` comes from
 * the local `files` table (persisted, backfilled from `/files/query`), so no
 * live server state rides in here.
 */
export function uploadStatus(
  entry: LocalClipboardEntry,
): string | undefined {
  const summary = entry.summary;
  if (!summary.fileCount) return undefined;
  // Content ids are computed in the background, so a fresh entry is usable
  // locally before it can be addressed on the server.
  if (isHashing(entry)) return `计算中 ${summary.hashedCount}/${summary.fileCount}`;
  if (!summary.contentCount) return undefined;
  const storedCount = summary.storedCount;
  if (storedCount >= summary.contentCount) return "已上传";
  if (storedCount) {
    return `部分上传（${storedCount}/${summary.contentCount}）`;
  }
  if (summary.uploadableSize !== undefined && summary.uploadableSize >= MANUAL_UPLOAD_LIMIT) {
    return "未上传（超过 100 MB）";
  }
  return "未上传";
}

export function fileEntrySummary(entry: LocalClipboardEntry): string | undefined {
  if (entry.kind !== "files" || !entry.summary.fileCount) return undefined;
  const count = `${entry.summary.fileCount} 个文件`;
  if (!entry.summary.totalSize) return count;
  return `${count} · ${formatFileSize(entry.summary.totalSize)}`;
}

export function canSaveEntry(entry: LocalClipboardEntry): boolean {
  return (entry.kind === "files" || entry.kind === "image")
    && entry.summary.contentCount > 0;
}

export function saveEntryLabel(entry: LocalClipboardEntry, savingEntryId: string, isMobile: boolean): string {
  if (savingEntryId === entry.id) return isMobile ? "正在下载…" : "正在另存为…";
  return isMobile ? "下载并保存…" : "另存为…";
}
