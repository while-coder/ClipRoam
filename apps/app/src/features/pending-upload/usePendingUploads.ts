import { accountStateRef, getAccountSession } from "../sync/accountSession";
import { showToast } from "../toast/useToast";
import { errorMessage } from "../../utils/error";
import type { LocalClipboardEntry } from "../../types";

export const pendingEntries = accountStateRef("pendingEntries");
export const pendingCount = accountStateRef("pendingCount");
export const uploadProgressByEntryId = accountStateRef("uploadProgress");

export function refreshPending(): void {
  const session = getAccountSession();
  session?.schedule("pending-refresh", () => {
    if (session.state.activeView.value === "pending-upload") void refreshPendingEntries();
    else void refreshPendingCount();
  }, 200);
}
export function cancelPendingRefresh(): void { getAccountSession()?.cancelScheduled("pending-refresh"); }

export async function removePendingEntry(entry: LocalClipboardEntry): Promise<void> {
  const session = getAccountSession();
  if (!session || !/^p\d+$/.test(entry.id)) return;
  try { await session.invoke("dequeue_pending_entry", { seq: Number(entry.id.slice(1)) }); }
  catch (error) { if (!session.signal.aborted) showToast(`删除失败：${errorMessage(error)}`, "error"); }
}

export async function refreshPendingEntries(): Promise<void> {
  const session = getAccountSession();
  if (!session) return;
  try {
    const entries = await session.invoke<LocalClipboardEntry[]>("list_pending_entries");
    session.state.pendingEntries.value = entries;
    session.state.pendingCount.value = entries.length;
  } catch (error) {
    if (!session.signal.aborted) showToast(`待上传记录读取失败：${errorMessage(error)}`, "error");
  }
}
export async function refreshPendingCount(): Promise<void> {
  const session = getAccountSession();
  if (!session) return;
  try { session.state.pendingCount.value = await session.invoke<number>("count_pending_entries"); }
  catch { /* Keep the existing badge on a transient read failure. */ }
}

export function queueUploadProgress(entryId: string, uploadedBytes: number, totalBytes: number): void {
  const session = getAccountSession();
  if (!session) return;
  session.state.pendingUploadProgress.set(entryId, { uploadedBytes, totalBytes });
  session.schedule("upload-progress", () => {
    const batch = Object.fromEntries(session.state.pendingUploadProgress);
    session.state.pendingUploadProgress.clear();
    session.state.uploadProgress.value = { ...session.state.uploadProgress.value, ...batch };
  }, 200);
}
export function finishUploadProgress(entryId: string): void {
  const session = getAccountSession();
  if (!session) return;
  session.state.pendingUploadProgress.delete(entryId);
  const { [entryId]: _, ...remaining } = session.state.uploadProgress.value;
  session.state.uploadProgress.value = remaining;
}
export function cancelUploadProgressFlush(): void {
  const session = getAccountSession();
  if (!session) return;
  session.cancelScheduled("upload-progress");
  session.state.pendingUploadProgress.clear();
  session.state.uploadProgress.value = {};
}
