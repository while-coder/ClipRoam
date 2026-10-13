import { computed } from "vue";
import { entryContents } from "@cliproam/protocol";
import { accountStateRef, getAccountSession } from "../sync/accountSession";
import { refreshHistory } from "../history/useHistorySync";
import { errorMessage } from "../../utils/error";
import type { LocalClipboardEntry } from "../../types";
import { isMobile } from "../../composables/usePlatform";
import { showToast } from "../toast/useToast";

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

export type DownloadRecord = {
  id: string;
  entryId: string;
  label: string;
  size: number;
  fileCount: number;
  status: "queued" | "downloading" | "saving" | "succeeded" | "failed" | "cancelled" | "interrupted";
  createdMs: number;
  finishedMs: number | null;
  receivedBytes: number;
  destination: string;
  error: string | null;
  files: DownloadTaskSnapshot[];
};

const storedDownloadRecords = accountStateRef("downloadRecords");
export const downloadRecords = computed<DownloadRecord[]>(() => storedDownloadRecords.value.map((record) => {
  if (record.finishedMs !== null || record.status === "saving") return record;
  const files = downloadTasks.value.filter((task) => task.requestIds.includes(record.id));
  if (!files.length) return record;
  return {
    ...record, files,
    status: files.every((file) => file.status === "queued") ? "queued" : "downloading",
    receivedBytes: files.reduce((sum, file) => sum + file.receivedBytes, 0),
  };
}));

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

/** Account-owned TS download queue snapshots. */
export const downloadTasks = accountStateRef("downloadTasks");
export const entryDownloadToasts = accountStateRef("entryDownloadToasts");

/** Enqueue each fileId and resolve only when all content is locally available. */
export async function downloadEntry(entryId: string, session = getAccountSession(), saveId?: string): Promise<void> {
  if (!session) throw new DownloadCancelledError("账号会话已结束");
  const entry = await session.client!.history.getEntry(entryId) as LocalClipboardEntry;
  // getEntry refreshes previewPath from disk, rather than the history list's stale summary.
  if (!saveId && entry.kind === "image" && entry.summary.previewPath) return;
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
    const outcome = await session.downloads.batch(entryId, call.id, saveId);
    if (outcome.cancelled) throw new DownloadCancelledError("已取消");
    if (outcome.failedCount > 0) throw new Error(`有 ${outcome.failedCount} 个文件下载失败（共 ${outcome.total} 个）`);
    update({ status: "succeeded" });
  } catch (error) {
    update({ status: error instanceof DownloadCancelledError || session.signal.aborted ? "cancelled" : "failed", error: errorMessage(error) });
    throw error;
  } finally {
    if (!session.signal.aborted) {
      refreshHistory();
      const current = session.state.entryDownloadToasts.value.find((item) => item.id === call.id);
      if (!isMobile.value || current?.status !== "failed") {
        session.schedule(`entry-download-toast-${call.id}`, () => dismissEntryDownloadToast(call.id), 5000);
      }
    }
  }
}

export async function cancelEntryDownloadCall(requestId: string): Promise<void> {
  const session = getAccountSession();
  const call = session?.state.entryDownloadToasts.value.find((item) => item.id === requestId);
  if (!session || !call || call.status !== "downloading" || !call.ready || call.cancelling) return;
  session.state.entryDownloadToasts.value = session.state.entryDownloadToasts.value.map((item) => item.id === requestId ? { ...item, cancelling: true } : item);
  try {
    session.downloads.cancelRequest(requestId);
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
  getAccountSession()?.downloads.cancelTask(taskId);
}

export function stopAllDownloads(reason?: string): void {
  void reason; getAccountSession()?.downloads.stop();
}

export async function refreshDownloadTasks(): Promise<void> {
  const session = getAccountSession();
  if (!session) return;
  const tasks = session.downloads.tasks();
  applyDownloadSnapshot(tasks);
  await refreshDownloadRecords();
}

export async function refreshDownloadRecords(): Promise<void> {
  const session = getAccountSession();
  if (!session) return;
  const revision = ++session.state.downloadRecordsRevision;
  const records = session.downloads.records();
  if (revision === session.state.downloadRecordsRevision) session.state.downloadRecords.value = records;
}

export async function removeDownloadRecords(recordId?: string): Promise<void> {
  const session = getAccountSession();
  if (!session) return;
  try {
    await session.downloads.removeRecords(recordId);
    await refreshDownloadRecords();
  } catch (error) {
    if (!session.signal.aborted) showToast(`清理下载记录失败：${errorMessage(error)}`, "error");
  }
}

export async function cancelDownloadRecord(recordId: string): Promise<void> {
  const session = getAccountSession();
  if (!session) return;
  try {
    await session.downloads.cancelRecord(recordId);
  } catch (error) {
    if (!session.signal.aborted) showToast(`取消下载失败：${errorMessage(error)}`, "error");
  }
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

/** 侧边栏「下载」入口的角标：未结束的下载批次数（包含保存阶段）。 */
export const activeDownloadCount = computed(() =>
  downloadRecords.value.filter((record) => record.finishedMs === null).length);

/** Download content then re-read the entry's local availability summary. */
export async function ensureLocalFiles(entry: LocalClipboardEntry, session = getAccountSession()): Promise<LocalClipboardEntry> {
  if (entry.kind !== "files" && entry.kind !== "image") return entry;
  if (!session) throw new DownloadCancelledError("账号会话已结束");
  await downloadEntry(entry.id, session);
  return (await session.client!.history.getEntry(entry.id)) as LocalClipboardEntry;
}
