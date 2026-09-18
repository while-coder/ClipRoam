import { computed, ref } from "vue";
import type { ServeTaskSnapshot } from "../sync/fileTransfer";
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
  const batch = pendingUploadProgress;
  pendingUploadProgress.clear();
  uploadProgressByEntryId.value = {
    ...uploadProgressByEntryId.value,
    ...Object.fromEntries(batch),
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
}

/** 「上传」页的中继应答任务快照：file.requested 且本机应答时记入（内存台账，随客户端重建清空）。 */
export const uploadTasks = ref<ServeTaskSnapshot[]>([]);
/** 侧边栏「上传」入口的角标：待发送 + 发送中的任务数。 */
export const activeUploadCount = computed(() =>
  uploadTasks.value.filter((task) => task.status === "pending" || task.status === "serving").length);
