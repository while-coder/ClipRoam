import { MANUAL_UPLOAD_LIMIT } from "../features/sync/syncClient";
import { runningInTauri } from "../composables/usePlatform";
import { formatFileSize } from "./format";
import type {
  ClipboardEntry,
  Device,
  DownloadProgress,
  LocalClipboardEntry,
  UploadProgress,
} from "../types";

/**
 * Pure display helpers shared by the history and pending-sync views. Anything
 * that reads reactive state takes it as an argument so both views can feed it
 * from their own props.
 */

export function deviceName(devicesById: Record<string, Device>, entry: ClipboardEntry): string {
  return devicesById[entry.sourceDeviceId]?.name ?? "未知设备";
}

export function isHashing(entry: LocalClipboardEntry): boolean {
  return entry.summary.hashedCount < entry.summary.fileCount;
}

export function uploadStatus(
  entry: LocalClipboardEntry,
  uploadProgress: Record<string, UploadProgress>,
  downloadProgress: Record<string, DownloadProgress>,
): string | undefined {
  const summary = entry.summary;
  if (!summary.fileCount) return undefined;
  // Content ids are computed in the background, so a fresh entry is usable
  // locally before it can be addressed on the server.
  if (isHashing(entry)) return `计算中 ${summary.hashedCount}/${summary.fileCount}`;
  const download = downloadProgress[entry.id];
  if (download) return `下载中 ${download.finished}/${download.total}`;
  const progress = uploadProgress[entry.id];
  if (progress) {
    const percent = progress.totalBytes
      ? Math.min(99, Math.floor((progress.uploadedBytes / progress.totalBytes) * 100))
      : 0;
    return `上传中 ${percent}%`;
  }
  if (!summary.contentCount) return undefined;
  if (summary.uploadedCount === summary.contentCount) return "已上传";
  if (summary.uploadedCount) {
    return `部分上传（${summary.uploadedCount}/${summary.contentCount}）`;
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
  return runningInTauri
    && (entry.kind === "files" || entry.kind === "image")
    && entry.summary.contentCount > 0;
}

export function saveEntryLabel(entry: LocalClipboardEntry, savingEntryId: string, isMobile: boolean): string {
  if (savingEntryId === entry.id) return isMobile ? "正在下载…" : "正在另存为…";
  return isMobile ? "下载到本机缓存" : "另存为…";
}

export function syncStatusLabel(synced: boolean): string {
  return synced ? "已同步到服务器" : "未同步到服务器";
}
