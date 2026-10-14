import { errorMessage } from "../../utils";
import { computed } from "vue";
import { getAccountSession } from "../../accountSession";
import type { LocalClipboardEntry } from "../../types";
import { isMobile, showNotification } from "../../platform/runtime";
import { DownloadCancelledError } from "./downloadManager";

export const downloadRecords = computed(() => getAccountSession()?.downloads.downloadRecords.value ?? []);
export const entryDownloadToasts = computed(() => getAccountSession()?.downloads.entryToasts.value ?? []);
export const activeDownloadCount = computed(() => getAccountSession()?.downloads.activeCount.value ?? 0);

/** Mobile failures stay visible until dismissed. The manager owns the download itself. */
export async function downloadEntry(entryId: string, session = getAccountSession(), saveId?: string): Promise<void> {
  if (!session) throw new DownloadCancelledError("账号会话已结束");
  await session.downloads.downloadEntry(entryId, saveId, isMobile.value);
}

export async function ensureLocalFiles(entry: LocalClipboardEntry, session = getAccountSession()): Promise<LocalClipboardEntry> {
  if (entry.kind !== "files" && entry.kind !== "image") return entry;
  if (!session) throw new DownloadCancelledError("账号会话已结束");
  return session.downloads.ensureLocalFiles(entry, isMobile.value);
}

export async function cancelEntryDownloadCall(requestId: string): Promise<void> {
  await getAccountSession()?.downloads.cancelEntryCall(requestId);
}
export function dismissEntryDownloadToast(requestId: string): void {
  getAccountSession()?.downloads.dismissEntryToast(requestId);
}
export function stopAllDownloads(): void { getAccountSession()?.downloads.stop(); }
export async function refreshDownloadTasks(): Promise<void> { getAccountSession()?.downloads.refresh(); }

export async function removeDownloadRecords(recordId?: string): Promise<void> {
  const session = getAccountSession();
  try { await session?.downloads.removeRecords(recordId); }
  catch (error) { if (!session?.signal.aborted) showNotification({ message: `清理下载记录失败：${errorMessage(error)}`, tone: "error" }); }
}
export async function cancelDownloadRecord(recordId: string): Promise<void> {
  const session = getAccountSession();
  try { await session?.downloads.cancelRecord(recordId); }
  catch (error) { if (!session?.signal.aborted) showNotification({ message: `取消下载失败：${errorMessage(error)}`, tone: "error" }); }
}
