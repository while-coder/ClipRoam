import { computed } from "vue";
import { entryContents } from "@cliproam/protocol";
import { accountStateRef, getAccountSession } from "../sync/accountSession";
import { refreshHistory } from "../history/useHistorySync";
import { errorMessage } from "../../utils/error";
import type { LocalClipboardEntry } from "../../types";

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
  requestIds: string[];
  /** 面板显示名：entryLabel + 序号，缺省用 fileId 前 8 位。 */
  label: string;
  size: number;
  status: DownloadTaskStatus;
  receivedBytes: number;
  error?: string;
};

export type EntryDownloadToast = {
  id: string;
  entryId: string;
  label: string;
  status: "downloading" | "succeeded" | "failed" | "cancelled";
  total: number;
  totalBytes: number;
  ready: boolean;
  cancelling: boolean;
  error?: string;
  files: Record<string, DownloadTaskSnapshot>;
};

type BatchOutcome = {
  cancelled: boolean;
  failedCount: number;
  total: number;
};

/** All windows share the Rust download queue; this is the current window's snapshot. */
export const downloadTasks = accountStateRef("downloadTasks");
export const entryDownloadToasts = accountStateRef("entryDownloadToasts");

/** Enqueue each fileId and resolve only when all content is locally available. */
export async function downloadEntry(entryId: string, session = getAccountSession()): Promise<void> {
  if (!session) throw new DownloadCancelledError("账号会话已结束");
  const entry = await session.client!.history.getEntry(entryId);
  const files = entryContents(entry);
  if (!files.length) return;
  const call: EntryDownloadToast = {
    id: crypto.randomUUID(), entryId, label: entry.content, status: "downloading",
    total: files.length, totalBytes: files.reduce((sum, file) => sum + file.size, 0),
    ready: false, cancelling: false, files: {},
  };
  session.state.entryDownloadToasts.value = [...session.state.entryDownloadToasts.value, call];
  const update = (patch: Partial<EntryDownloadToast>) => {
    session.state.entryDownloadToasts.value = session.state.entryDownloadToasts.value.map((item) =>
      item.id === call.id ? { ...item, ...patch } : item);
  };
  try {
    const outcome = await session.invoke<BatchOutcome>("download_entry", { entryId, requestId: call.id });
    if (outcome.cancelled) throw new DownloadCancelledError("已取消");
    if (outcome.failedCount > 0) throw new Error(`有 ${outcome.failedCount} 个文件下载失败（共 ${outcome.total} 个）`);
    update({ status: "succeeded" });
  } catch (error) {
    update({ status: error instanceof DownloadCancelledError || session.signal.aborted ? "cancelled" : "failed", error: errorMessage(error) });
    throw error;
  } finally {
    if (!session.signal.aborted) {
      refreshHistory();
      session.schedule(`entry-download-toast-${call.id}`, () => dismissEntryDownloadToast(call.id), 5000);
    }
  }
}

export async function cancelEntryDownloadCall(requestId: string): Promise<void> {
  const session = getAccountSession();
  const call = session?.state.entryDownloadToasts.value.find((item) => item.id === requestId);
  if (!session || !call || call.status !== "downloading" || !call.ready || call.cancelling) return;
  session.state.entryDownloadToasts.value = session.state.entryDownloadToasts.value.map((item) => item.id === requestId ? { ...item, cancelling: true } : item);
  try {
    await session.invoke("cancel_entry_download_call", { requestId });
  } catch (error) {
    if (!session.signal.aborted) session.state.entryDownloadToasts.value = session.state.entryDownloadToasts.value.map((item) =>
      item.id === requestId && item.status === "downloading" ? { ...item, cancelling: false, error: errorMessage(error) } : item);
  }
}

export function dismissEntryDownloadToast(requestId: string): void {
  const session = getAccountSession();
  if (!session) return;
  session.state.entryDownloadToasts.value = session.state.entryDownloadToasts.value.filter((item) => item.id !== requestId || item.status === "downloading");
}

export function cancelDownload(taskId: string): void {
  void getAccountSession()?.invoke("cancel_download", { taskId }).catch(() => undefined);
}

export function stopAllDownloads(reason?: string): void {
  void getAccountSession()?.invoke("stop_all_downloads", { reason }).catch(() => undefined);
}

export async function refreshDownloadTasks(): Promise<void> {
  const session = getAccountSession();
  if (!session) return;
  const tasks = await session.invoke<DownloadTaskSnapshot[]>("download_tasks");
  applyDownloadSnapshot(tasks);
}

export function applyDownloadSnapshot(payload: DownloadTaskSnapshot[]): void {
  const session = getAccountSession();
  if (!session) return;
  const tasks = payload.filter((task) => task.accountKey === session.accountKey);
  session.state.downloadTasks.value = tasks;
  session.state.entryDownloadToasts.value = session.state.entryDownloadToasts.value.map((call) => {
    if (call.status !== "downloading") return call;
    const updates = tasks.filter((task) => task.requestIds.includes(call.id));
    if (!updates.length) return call;
    return { ...call, ready: true, files: { ...call.files, ...Object.fromEntries(updates.map((task) => [task.fileId, task])) } };
  });
}

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
