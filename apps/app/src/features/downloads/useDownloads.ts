import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { fullEntry, refreshHistory } from "../history/useHistorySync";
import type { DownloadProgress, LocalClipboardEntry, MissingFile } from "../../types";

/** 取消专用哨兵：被取消的批次（含去重合并方连带取消）抛出，调用方据此静默收尾。 */
export class DownloadCancelledError extends Error {}

type DownloadTaskStatus = "queued" | "downloading" | "succeeded" | "failed" | "cancelled";

/** Rust `cliproam://download-changed` 事件推送的任务快照，字段一一对应。 */
export type DownloadTaskSnapshot = {
  id: string;
  /** 每次 downloadFiles() 自增；同 entry 重复批次按此归代，派生进度只看最新批。 */
  batchId: number;
  entryId: string;
  fileId: string;
  saveId?: string;
  /** 面板显示名：entryLabel + 序号，缺省用 fileId 前 8 位。 */
  label: string;
  size: number;
  status: DownloadTaskStatus;
  receivedBytes: number;
  error?: string;
};

type DownloadRequest = {
  fileId: string;
  size: number;
};

type BatchOutcome = {
  cancelled: boolean;
  failedCount: number;
  total: number;
};

/** All windows share the Rust download queue; this is the current window's snapshot. */
export const downloadTasks = ref<DownloadTaskSnapshot[]>([]);

export async function downloadFiles(
  entryId: string,
  files: readonly DownloadRequest[],
  options: { saveId?: string; entryLabel?: string } = {},
): Promise<void> {
  const outcome = await invoke<BatchOutcome>("download_files", { entryId, files, ...options });
  if (outcome.cancelled) throw new DownloadCancelledError("已取消");
  if (outcome.failedCount > 0) throw new Error(`有 ${outcome.failedCount} 个文件下载失败（共 ${outcome.total} 个）`);
}

export function cancelDownload(taskId: string): void {
  void invoke("cancel_download", { taskId }).catch(() => undefined);
}

export async function cancelEntryDownloads(entryId: string): Promise<number> {
  return invoke<number>("cancel_entry_downloads", { entryId });
}

export function stopAllDownloads(reason?: string): void {
  void invoke("stop_all_downloads", { reason }).catch(() => undefined);
}

export async function refreshDownloadTasks(): Promise<void> {
  applyDownloadSnapshot(await invoke<DownloadTaskSnapshot[]>("download_tasks"));
}

export function applyDownloadSnapshot(payload: DownloadTaskSnapshot[]): void {
  downloadTasks.value = payload;
}

/** 派生的逐条目下载进度：按 (entryId, batchId) 一次分组聚合——同批全部任务
 * 参与统计（含已成功的），只有批内仍有活动任务的批次才输出；避免逐任务
 * 全表扫描、同批重复计算。 */
export const downloadProgressByEntryId = computed<Record<string, DownloadProgress>>(() => {
  const batches = new Map<string, DownloadTaskSnapshot[]>();
  for (const task of downloadTasks.value) {
    const key = `${task.entryId}#${task.batchId}`;
    const group = batches.get(key);
    if (group) group.push(task);
    else batches.set(key, [task]);
  }
  const progress: Record<string, DownloadProgress> = {};
  for (const group of batches.values()) {
    if (!group.some((task) => task.status === "queued" || task.status === "downloading")) continue;
    progress[group[0]!.entryId] = {
      finished: group.filter((task) => task.status === "succeeded").length,
      total: group.length,
      receivedBytes: group.reduce((sum, task) => sum + task.receivedBytes, 0),
      totalBytes: group.reduce((sum, task) => sum + task.size, 0),
    };
  }
  return progress;
});
/** 侧边栏「下载」入口的角标：排队 + 下载中的任务数。 */
export const activeDownloadCount = computed(() =>
  downloadTasks.value.filter((task) => task.status === "queued" || task.status === "downloading").length);

/**
 * Fetches every content this device is missing. All windows go through the
 * shared Rust queue; credentials and the HTTP pull itself
 * live in the Rust-side downloader.
 */
export async function downloadRequiredFiles(
  entry: LocalClipboardEntry,
  prepareCommand: "prepare_entry_files" | "prepare_paste_entry",
): Promise<LocalClipboardEntry> {
  if (entry.kind !== "files" && entry.kind !== "image") return entry;
  const missing = await invoke<MissingFile[]>(prepareCommand, { entryId: entry.id });
  if (!missing.length) return entry;
  try {
    await downloadFiles(entry.id, missing, { entryLabel: entry.content });
  } finally {
    refreshHistory();
  }
  // Re-read the persisted entry: its availability summary changed on disk.
  return (await fullEntry(entry)) as LocalClipboardEntry;
}

export async function ensureLocalFiles(entry: LocalClipboardEntry): Promise<LocalClipboardEntry> {
  return downloadRequiredFiles(entry, "prepare_entry_files");
}

export async function ensurePasteReady(entry: LocalClipboardEntry): Promise<LocalClipboardEntry> {
  return downloadRequiredFiles(entry, "prepare_paste_entry");
}
