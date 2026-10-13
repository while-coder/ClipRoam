import { computed } from "vue";
import { getAccountSession } from "../sync/accountSession";
import { showToast } from "../../core/toast";
import { errorMessage } from "../../utils/error";
import type { LocalClipboardEntry } from "../../types";

/** UI bindings follow the active account's shared manager. */
export const pendingEntries = computed(() => getAccountSession()?.pendingUploads.entries.value ?? []);
export const pendingCount = computed(() => getAccountSession()?.pendingUploads.count.value ?? 0);
export const uploadProgressByEntryId = computed(() => getAccountSession()?.pendingUploads.progress.value ?? {});

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
  try { await session?.pendingUploads.remove(entry.id); }
  catch (error) { if (!session?.signal.aborted) showToast(`删除失败：${errorMessage(error)}`, "error"); }
}

export async function retryPendingEntry(entry: LocalClipboardEntry): Promise<void> {
  const session = getAccountSession();
  try { await session?.pendingUploads.retry(entry.id); }
  catch (error) { if (!session?.signal.aborted) showToast(`重试失败：${errorMessage(error)}`, "error"); }
}

export async function refreshPendingEntries(): Promise<void> {
  const session = getAccountSession();
  try { await session?.pendingUploads.refresh(); }
  catch (error) { if (!session?.signal.aborted) showToast(`待上传记录读取失败：${errorMessage(error)}`, "error"); }
}

export async function refreshPendingCount(): Promise<void> {
  try { await getAccountSession()?.pendingUploads.refresh(false); }
  catch { /* Keep the existing badge on a transient read failure. */ }
}

export function cancelUploadProgressFlush(): void { getAccountSession()?.pendingUploads.clearProgress(); }
