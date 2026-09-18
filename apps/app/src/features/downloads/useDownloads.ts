import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { Downloader, type DownloadTaskSnapshot } from "../sync/fileTransfer";
import { fullEntry, refreshHistory } from "../clipboard-history/useHistorySync";
import type { DownloadProgress, LocalClipboardEntry, MissingFile } from "../../types";

/** 下载面板的任务快照；Rust Downloader 事件推来时由薄桥整体替换。 */
export const downloadTasks = ref<DownloadTaskSnapshot[]>([]);
export const downloader = new Downloader({
  onTasksChanged: () => { downloadTasks.value = [...downloader.tasksSnapshot()]; },
});
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
 * shared Downloader; queueing, credentials and the HTTP pull itself
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
    await downloader.downloadFiles(entry.id, missing, { entryLabel: entry.content });
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
