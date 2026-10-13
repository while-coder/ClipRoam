import type { ClipboardEntry, Device, LocalClipboardEntry } from "./types";

// 取消信号

/** Combine cancellation signals on WebViews that lack AbortSignal.any. */
export function anyAbortSignal(signals: AbortSignal[]): AbortSignal {
  if (typeof AbortSignal.any === "function") return AbortSignal.any(signals);

  const controller = new AbortController();
  const alreadyAborted = signals.find((signal) => signal.aborted);
  if (alreadyAborted) {
    controller.abort(alreadyAborted.reason);
    return controller.signal;
  }

  const listeners = new Map<AbortSignal, () => void>();
  for (const signal of new Set(signals)) {
    const onAbort = () => {
      controller.abort(signal.reason);
      for (const [source, listener] of listeners) {
        source.removeEventListener("abort", listener);
      }
      listeners.clear();
    };
    listeners.set(signal, onAbort);
    signal.addEventListener("abort", onAbort, { once: true });
  }
  return controller.signal;
}

// 字节转换

export function base64ToBytes(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 8192) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
  }
  return btoa(binary);
}

// 错误信息

/** The message of an unknown thrown value, for toasts and error strings. */
export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

// 日期、大小与进度格式化

/** Pure date/size formatting helpers shared by every window. */

/** Labels for the preset time filters; TimeFilterControl renders the same strings. */
export const TIME_FILTER_LABELS: Record<string, string> = {
  today: "今天",
  "7-days": "近 7 天",
  "30-days": "近 30 天",
};

export function formatAge(createdAt: string, now: number): string {
  const elapsed = Math.max(0, now - new Date(createdAt).getTime());
  const seconds = Math.floor(elapsed / 1_000);
  if (seconds < 10) return "刚刚";
  if (seconds < 60) return `${seconds} 秒前`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  return new Intl.DateTimeFormat("zh-CN", { month: "short", day: "numeric" }).format(new Date(createdAt));
}

export function formatExactDateTime(createdAt: string | number): string {
  const date = new Date(createdAt);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

export function parseLocalDate(value: string, endOfDay = false): Date | undefined {
  const [year, month, day] = value.split("-").map(Number);
  if (!year || !month || !day) return undefined;
  return new Date(year, month - 1, day, endOfDay ? 23 : 0, endOfDay ? 59 : 0, endOfDay ? 59 : 0, endOfDay ? 999 : 0);
}

/** Shared custom-date-range validation; returns the error message, or "" when valid. */
export function validateDateRange(start: string, end: string): string {
  if (!start || !end) return "请选择完整的开始和结束日期";
  if (start > end) return "开始日期不能晚于结束日期";
  return "";
}

// 与 apps/admin/src/sections/files/FilesSection.vue 的 formatBytes 是同一实现，改动请同步。
export function formatFileSize(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

/** cur/total 的整数百分比，clamp 到 cap（进行中默认 99 封顶，到位由终态接管）；total 无效时为 0。 */
export function percentOf(cur: number, total: number, cap = 100): number {
  if (!total) return 0;
  return Math.min(cap, Math.floor((cur / total) * 100));
}

// 剪贴板条目展示

const MANUAL_UPLOAD_LIMIT = 100 * 1024 * 1024;

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

export function saveEntryLabel(entry: LocalClipboardEntry, savingEntryId: string): string {
  return savingEntryId === entry.id ? "正在下载…" : "下载";
}
