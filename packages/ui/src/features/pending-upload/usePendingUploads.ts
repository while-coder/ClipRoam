import { errorMessage } from "../../utils";
import { computed, ref } from "vue";
import { getAccountSession } from "../../accountSession";
import { showNotification } from "../../platform/runtime";
import type { LocalClipboardEntry } from "../../types";
import type { FilePickerMode } from "../../platform/native";

/** UI bindings follow the active account's shared manager. */
export const pendingEntries = computed(() => getAccountSession()?.pendingUploads.entries.value ?? []);
export const pendingCount = computed(() => getAccountSession()?.pendingUploads.count.value ?? 0);
export const uploadProgressByEntryId = computed(() => getAccountSession()?.pendingUploads.progress.value ?? {});
export const retryingAllPending = computed(() => getAccountSession()?.pendingUploads.retryingAll.value ?? false);

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
  catch (error) { if (!session?.signal.aborted) showNotification({ message: `删除失败：${errorMessage(error)}`, tone: "error" }); }
}

export async function retryPendingEntry(entry: LocalClipboardEntry): Promise<void> {
  const session = getAccountSession();
  try { await session?.pendingUploads.retry(entry.id); }
  catch (error) { if (!session?.signal.aborted) showNotification({ message: `重试失败：${errorMessage(error)}`, tone: "error" }); }
}

export async function retryAllPendingEntries(): Promise<void> {
  const session = getAccountSession();
  try { await session?.pendingUploads.retryAll(); }
  catch (error) { if (!session?.signal.aborted) showNotification({ message: `全部重试失败：${errorMessage(error)}`, tone: "error" }); }
}

export async function refreshPendingEntries(): Promise<void> {
  const session = getAccountSession();
  try { await session?.pendingUploads.refresh(); }
  catch (error) { if (!session?.signal.aborted) showNotification({ message: `待上传记录读取失败：${errorMessage(error)}`, tone: "error" }); }
}

export async function refreshPendingCount(): Promise<void> {
  try { await getAccountSession()?.pendingUploads.refresh(false); }
  catch { /* Keep the existing badge on a transient read failure. */ }
}

export function cancelUploadProgressFlush(): void { getAccountSession()?.pendingUploads.clearProgress(); }

// Shared across workspace pages so switching pages cannot open another picker.
export const capturingClipboard = ref(false);
export const uploadingFiles = ref(false);

export async function captureCurrentClipboard(): Promise<void> {
  if (capturingClipboard.value) return;
  capturingClipboard.value = true;
  try {
    const captured = await getAccountSession()?.capture.readCurrentText();
    showNotification({ message: captured ? "已读取当前文本剪贴板" : "当前剪贴板没有可读取的文本", tone: captured ? "success" : "info" });
  } catch (error) {
    showNotification({ message: `读取剪贴板失败：${errorMessage(error)}`, tone: "error" });
  } finally {
    capturingClipboard.value = false;
  }
}

export async function captureFilesFromPicker(mode: FilePickerMode): Promise<void> {
  if (uploadingFiles.value) return;
  uploadingFiles.value = true;
  try {
    const captured = await getAccountSession()?.capture.pickFiles(mode);
    showNotification({ message: captured ? "已加入上传队列" : "未选择文件", tone: captured ? "success" : "info" });
  } catch (error) {
    showNotification({ message: `选择文件失败：${errorMessage(error)}`, tone: "error" });
  } finally {
    uploadingFiles.value = false;
  }
}
