import { computed } from "vue";
import { accountStateRef, getAccountSession } from "../sync/accountSession";
import { refreshHistory } from "../history/useHistorySync";
import type { DownloadProgress, LocalClipboardEntry } from "../../types";

/** 取消专用哨兵：被取消的批次（含去重合并方连带取消）抛出，调用方据此静默收尾。 */
export class DownloadCancelledError extends Error {}

type DownloadTaskStatus = "queued" | "downloading" | "succeeded" | "failed" | "cancelled";

/** Rust `cliproam://download-changed` 事件推送的任务快照，字段一一对应。 */
export type DownloadTaskSnapshot = {
  id: string;
  accountKey: string;
  /** 任务创建的批次序号；同一 fileId 的重复任务只统计最新一次。 */
  batchId: number;
  entryId: string;
  fileId: string;
  entryIds: string[];
  /** 面板显示名：entryLabel + 序号，缺省用 fileId 前 8 位。 */
  label: string;
  size: number;
  status: DownloadTaskStatus;
  receivedBytes: number;
  error?: string;
};

type BatchOutcome = {
  cancelled: boolean;
  failedCount: number;
  total: number;
};

/** All windows share the Rust download queue; this is the current window's snapshot. */
export const downloadTasks = accountStateRef("downloadTasks");

/** Enqueue each fileId and resolve only when all content is locally available. */
export async function downloadEntry(entryId: string, session = getAccountSession()): Promise<void> {
  if (!session) throw new DownloadCancelledError("账号会话已结束");
  try {
    const outcome = await session.invoke<BatchOutcome>("download_entry", { entryId });
    if (outcome.cancelled) throw new DownloadCancelledError("已取消");
    if (outcome.failedCount > 0) throw new Error(`有 ${outcome.failedCount} 个文件下载失败（共 ${outcome.total} 个）`);
  } finally {
    if (!session.signal.aborted) refreshHistory();
  }
}

export function cancelDownload(taskId: string): void {
  void getAccountSession()?.invoke("cancel_download", { taskId }).catch(() => undefined);
}

export async function cancelEntryDownloads(entryId: string): Promise<number> {
  return getAccountSession()?.invoke<number>("cancel_entry_downloads", { entryId }) ?? 0;
}

export function stopAllDownloads(reason?: string): void {
  void getAccountSession()?.invoke("stop_all_downloads", { reason }).catch(() => undefined);
}

export async function refreshDownloadTasks(): Promise<void> {
  const session = getAccountSession();
  if (!session) return;
  const tasks = await session.invoke<DownloadTaskSnapshot[]>("download_tasks");
  session.state.downloadTasks.value = tasks.filter((task) => task.accountKey === session.accountKey);
}

export function applyDownloadSnapshot(payload: DownloadTaskSnapshot[]): void {
  const session = getAccountSession();
  if (session) session.state.downloadTasks.value = payload.filter((task) => task.accountKey === session.accountKey);
}

/** A shared file contributes progress to every entry waiting for it. */
export const downloadProgressByEntryId = computed<Record<string, DownloadProgress>>(() => {
  const entries = new Map<string, Map<string, DownloadTaskSnapshot>>();
  for (const task of downloadTasks.value) {
    for (const entryId of task.entryIds) {
      let files = entries.get(entryId);
      if (!files) { files = new Map(); entries.set(entryId, files); }
      const previous = files.get(task.fileId);
      if (!previous || previous.batchId < task.batchId) files.set(task.fileId, task);
    }
  }
  const progress: Record<string, DownloadProgress> = {};
  for (const [entryId, files] of entries) {
    const tasks = [...files.values()];
    if (!tasks.some((task) => task.status === "queued" || task.status === "downloading")) continue;
    progress[entryId] = {
      finished: tasks.filter((task) => task.status === "succeeded").length,
      total: tasks.length,
      receivedBytes: tasks.reduce((sum, task) => sum + task.receivedBytes, 0),
      totalBytes: tasks.reduce((sum, task) => sum + task.size, 0),
    };
  }
  return progress;
});
/** 侧边栏「下载」入口的角标：排队 + 下载中的任务数。 */
export const activeDownloadCount = computed(() =>
  downloadTasks.value.filter((task) => task.status === "queued" || task.status === "downloading").length);

/** Download content then re-read the entry's local availability summary. */
export async function ensureLocalFiles(entry: LocalClipboardEntry, session = getAccountSession()): Promise<LocalClipboardEntry> {
  if (entry.kind !== "files" && entry.kind !== "image") return entry;
  if (!session) throw new DownloadCancelledError("账号会话已结束");
  await downloadEntry(entry.id, session);
  return (await session.client!.history.getEntry(entry.id)) as LocalClipboardEntry;
}
