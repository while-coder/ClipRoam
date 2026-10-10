import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { activeView } from "../../composables/useActiveView";
import { showToast } from "../toast/useToast";
import { errorMessage } from "../../utils/error";
import type { LocalClipboardEntry, UploadProgress } from "../../types";

/**
 * The durable upload queue's rows: captured offline or waiting to publish.
 * Served by Rust's `list_pending_entries` (every queue row as an entry-shaped
 * view with a temporary `p{seq}` id) — never derived from a whole-history read.
 * Details load only while the pending-upload view is open; refresh bursts
 * elsewhere carry the O(1) `count_pending_entries` instead.
 */
export const pendingEntries = ref<LocalClipboardEntry[]>([]);
export const pendingCount = ref(0);

let refreshTimer: number | undefined;

/** Only queue mutations invalidate the pending list and its badge. */
export function refreshPending(): void {
  if (refreshTimer !== undefined) return;
  refreshTimer = window.setTimeout(() => {
    refreshTimer = undefined;
    if (activeView.value === "pending-upload") void refreshPendingEntries();
    else void refreshPendingCount();
  }, 200);
}

export function cancelPendingRefresh(): void {
  if (refreshTimer !== undefined) window.clearTimeout(refreshTimer);
  refreshTimer = undefined;
}

/** A pending row only exists in the local queue, never in server history. */
export async function removePendingEntry(entry: LocalClipboardEntry): Promise<void> {
  if (!/^p\d+$/.test(entry.id)) return;
  const seq = Number(entry.id.slice(1));
  await invoke("dequeue_pending_entry", { seq }).catch((error) => {
    showToast(`删除失败：${errorMessage(error)}`, "error");
  });
}

/** The pending-upload list re-queries Rust on every refresh. */
export async function refreshPendingEntries(): Promise<void> {
  try {
    pendingEntries.value = await invoke<LocalClipboardEntry[]>("list_pending_entries");
    pendingCount.value = pendingEntries.value.length;
  } catch (error) {
    showToast(`待上传记录读取失败：${errorMessage(error)}`, "error");
  }
}

/**
 * Sidebar badge only: an O(1) count, so refresh bursts outside the pending
 * view never haul the queue rows across the IPC boundary.
 */
export async function refreshPendingCount(): Promise<void> {
  try {
    pendingCount.value = await invoke<number>("count_pending_entries");
  } catch {
    // The badge is auxiliary; a failed refresh keeps the previous value.
  }
}

export const uploadProgressByEntryId = ref<Record<string, UploadProgress>>({});
/**
 * 上传进度节流：chunk 回调频率高（多路并发叠加），逐次展开响应式 record
 * 会放大 GC 与整列表重渲染。chunk 只更新待写缓存，200ms 合并一次提交；
 * 上传结束走立即路径，避免缓存里的旧值把删除动作覆盖回去。
 */
const UPLOAD_PROGRESS_THROTTLE_MS = 200;
const pendingUploadProgress = new Map<string, UploadProgress>();
let uploadProgressFlushTimer: number | undefined;

function flushUploadProgress(): void {
  uploadProgressFlushTimer = undefined;
  if (!pendingUploadProgress.size) return;
  const batch = Object.fromEntries(pendingUploadProgress);
  pendingUploadProgress.clear();
  uploadProgressByEntryId.value = {
    ...uploadProgressByEntryId.value,
    ...batch,
  };
}

export function queueUploadProgress(entryId: string, uploadedBytes: number, totalBytes: number): void {
  pendingUploadProgress.set(entryId, { uploadedBytes, totalBytes });
  if (uploadProgressFlushTimer === undefined) {
    uploadProgressFlushTimer = window.setTimeout(flushUploadProgress, UPLOAD_PROGRESS_THROTTLE_MS);
  }
}

export function finishUploadProgress(entryId: string): void {
  pendingUploadProgress.delete(entryId);
  uploadProgressByEntryId.value = withoutKey(uploadProgressByEntryId.value, entryId);
}

function withoutKey<T>(record: Record<string, T>, id: string): Record<string, T> {
  const { [id]: _, ...remaining } = record;
  return remaining;
}

export function cancelUploadProgressFlush(): void {
  if (uploadProgressFlushTimer !== undefined) window.clearTimeout(uploadProgressFlushTimer);
  uploadProgressFlushTimer = undefined;
  pendingUploadProgress.clear();
  uploadProgressByEntryId.value = {};
}
