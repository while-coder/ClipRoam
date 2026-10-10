import { ref } from "vue";
import type { UploadProgress } from "../../types";

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
}

