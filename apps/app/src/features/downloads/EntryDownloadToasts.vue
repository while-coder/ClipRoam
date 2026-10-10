<script setup lang="ts">
import { Download, X } from "lucide-vue-next";
import { cancelEntryDownloadCall, dismissEntryDownloadToast, entryDownloadToasts, type EntryDownloadToast } from "./useDownloads";
import { formatFileSize, percentOf } from "../../utils/format";

function received(call: EntryDownloadToast): number {
  if (call.status === "succeeded") return call.totalBytes;
  return Object.values(call.files).reduce((sum, file) => sum + file.receivedBytes, 0);
}

function finished(call: EntryDownloadToast): number {
  if (call.status === "succeeded") return call.total;
  return Object.values(call.files).filter((file) => file.status === "succeeded").length;
}

function percent(call: EntryDownloadToast): number {
  return call.status === "succeeded" ? 100 : percentOf(received(call), call.totalBytes, 99);
}

function statusText(call: EntryDownloadToast): string {
  if (call.status === "succeeded") return "下载完成";
  if (call.status === "failed") return "下载失败";
  if (call.status === "cancelled") return "已取消";
  if (call.cancelling) return "正在取消";
  return call.ready ? "正在准备文件" : "等待下载";
}
</script>

<template>
  <aside v-if="entryDownloadToasts.length" class="entry-download-toasts" aria-label="条目下载列表">
    <article v-for="call in entryDownloadToasts" :key="call.id" class="entry-download-toast" :class="call.status">
      <Download :size="18" class="entry-download-icon" aria-hidden="true" />
      <div class="entry-download-body">
        <strong class="entry-download-label" :title="call.label">{{ call.label }}</strong>
        <span class="entry-download-state" role="status">{{ statusText(call) }} · {{ finished(call) }}/{{ call.total }} 个文件</span>
        <div v-if="call.status === 'downloading' || call.status === 'succeeded'" class="download-bar"
          role="progressbar" :aria-label="`${call.label} 下载进度`" :aria-valuenow="percent(call)" :aria-valuemin="0" :aria-valuemax="100">
          <span class="download-bar-fill" :style="{ width: `${percent(call)}%` }"></span>
        </div>
        <span class="entry-download-bytes">{{ formatFileSize(received(call)) }} / {{ formatFileSize(call.totalBytes) }}</span>
        <span v-if="call.error && call.status !== 'cancelled'" class="entry-download-error">{{ call.error }}</span>
      </div>
      <button v-if="call.status === 'downloading'" class="entry-download-action" type="button"
        :disabled="!call.ready || call.cancelling" :aria-label="`取消本次 ${call.label} 下载`" @click="cancelEntryDownloadCall(call.id)">取消</button>
      <button v-else class="entry-download-action" type="button" :aria-label="`关闭 ${call.label} 下载通知`" @click="dismissEntryDownloadToast(call.id)">
        <X :size="16" aria-hidden="true" />
      </button>
    </article>
  </aside>
</template>

<style scoped>
.entry-download-toasts {
  position: fixed;
  z-index: var(--sui-z-toast);
  right: max(12px, env(safe-area-inset-right));
  bottom: max(12px, env(safe-area-inset-bottom));
  display: flex;
  flex-direction: column;
  gap: 8px;
  width: min(360px, calc(100vw - 24px));
  max-height: min(60vh, 480px);
  overflow-y: auto;
  overscroll-behavior: contain;
}
.entry-download-toast {
  display: flex;
  flex-shrink: 0;
  align-items: flex-start;
  gap: 8px;
  padding: 12px;
  color: var(--color-foreground);
  background: var(--color-surface);
  border: 1px solid var(--border);
  border-radius: 10px;
  box-shadow: 0 8px 24px var(--sui-mask);
}
.entry-download-icon { flex-shrink: 0; margin-top: 3px; color: var(--color-secondary-text); }
.entry-download-body { display: grid; flex: 1; min-width: 0; gap: 6px; }
.entry-download-label { overflow: hidden; font-size: 13px; text-overflow: ellipsis; white-space: nowrap; }
.entry-download-state, .entry-download-bytes { color: var(--color-secondary-text); font-size: 12px; }
.entry-download-error { font-size: 12px; overflow-wrap: anywhere; }
.entry-download-toast.succeeded { border-color: var(--color-accent); }
.entry-download-action {
  display: grid;
  flex-shrink: 0;
  place-items: center;
  min-width: 44px;
  min-height: 44px;
  padding: 0 6px;
  background: transparent;
  border: 1px solid var(--border);
  border-radius: 6px;
  cursor: pointer;
  font-size: 12px;
}
.entry-download-action:hover:not(:disabled) { background: var(--sui-bg-hover); }
.entry-download-action:active:not(:disabled) { background: var(--sui-bg-active); }
@media (prefers-reduced-motion: reduce) {
  .download-bar-fill { transition: none; }
}
</style>
