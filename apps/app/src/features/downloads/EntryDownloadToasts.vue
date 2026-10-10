<script setup lang="ts">
import { ChevronDown, ChevronUp, Download, X } from "lucide-vue-next";
import { computed, ref, watch } from "vue";
import { cancelEntryDownloadCall, dismissEntryDownloadToast, entryDownloadToasts, type EntryDownloadToast } from "./useDownloads";
import { formatFileSize, percentOf } from "../../utils/format";

const props = defineProps<{ mobile?: boolean }>();
const expanded = ref(false);
const activeCount = computed(() => entryDownloadToasts.value.filter((call) => call.status === "downloading").length);
const failedCount = computed(() => entryDownloadToasts.value.filter((call) => call.status === "failed").length);
const summary = computed(() => {
  const parts = [];
  if (activeCount.value) parts.push(`正在下载 ${activeCount.value} 个条目`);
  if (failedCount.value) parts.push(`${failedCount.value} 个失败`);
  return parts.join(" · ") || "条目下载已结束";
});
watch(() => entryDownloadToasts.value.length, (count) => {
  if (!count) expanded.value = false;
});

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
  if (Object.values(call.files).some((file) => file.status === "downloading")) return "正在下载";
  return call.ready ? "正在准备文件" : "等待下载";
}
</script>

<template>
  <aside v-if="entryDownloadToasts.length" class="entry-download-toasts" :class="{ 'mobile-download-panel': props.mobile }" aria-label="条目下载列表">
    <button v-if="props.mobile" class="mobile-download-toggle" type="button" :aria-expanded="expanded"
      aria-controls="mobile-entry-download-list" @click="expanded = !expanded">
      <Download :size="18" aria-hidden="true" />
      <span role="status">{{ summary }}</span>
      <span class="mobile-download-toggle-label">{{ expanded ? '收起' : '展开' }}</span>
      <component :is="expanded ? ChevronDown : ChevronUp" :size="18" aria-hidden="true" />
    </button>
    <div v-show="!props.mobile || expanded" :id="props.mobile ? 'mobile-entry-download-list' : undefined" class="entry-download-list">
    <article v-for="call in entryDownloadToasts" :key="call.id" class="entry-download-toast" :class="call.status">
      <Download :size="18" class="entry-download-icon" aria-hidden="true" />
      <div class="entry-download-body">
        <strong class="entry-download-label" :title="call.label">{{ call.label }}</strong>
        <span class="entry-download-state" role="status">{{ statusText(call) }} · {{ finished(call) }}/{{ call.total }} 个文件</span>
        <div v-if="call.status === 'downloading' || call.status === 'succeeded'" class="download-bar"
          role="progressbar" :aria-label="`${call.label} 下载进度`" :aria-valuenow="percent(call)" :aria-valuemin="0" :aria-valuemax="100">
          <span class="download-bar-fill" :style="{ width: `${percent(call)}%` }"></span>
        </div>
        <span class="entry-download-bytes">{{ formatFileSize(received(call)) }} / {{ formatFileSize(call.totalBytes) }}<template v-if="props.mobile"> · {{ percent(call) }}%</template></span>
        <span v-if="call.error && call.status !== 'cancelled'" class="entry-download-error">{{ call.error }}</span>
      </div>
      <button v-if="call.status === 'downloading'" class="entry-download-action" type="button"
        :disabled="!call.ready || call.cancelling" :aria-label="`取消本次 ${call.label} 下载`" @click="cancelEntryDownloadCall(call.id)">取消</button>
      <button v-else class="entry-download-action" type="button" :aria-label="`关闭 ${call.label} 下载通知`" @click="dismissEntryDownloadToast(call.id)">
        <X :size="16" aria-hidden="true" />
      </button>
    </article>
    </div>
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
  overflow: hidden;
}
.entry-download-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
}
.mobile-download-panel {
  position: static;
  z-index: auto;
  grid-row: 2;
  width: auto;
  min-width: 0;
  max-height: min(45dvh, 360px);
  gap: 0;
  background: var(--color-surface);
  border-top: 1px solid var(--border);
}
.mobile-download-toggle {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: 8px;
  width: 100%;
  min-height: 48px;
  padding: 8px max(12px, env(safe-area-inset-right)) 8px max(12px, env(safe-area-inset-left));
  color: var(--color-foreground);
  background: transparent;
  border: 0;
  cursor: pointer;
  font-size: 13px;
  text-align: left;
  touch-action: manipulation;
}
.mobile-download-toggle > span:first-of-type { flex: 1; min-width: 0; overflow-wrap: anywhere; }
.mobile-download-toggle > svg { flex-shrink: 0; }
.mobile-download-toggle-label { color: var(--color-secondary-text); }
.mobile-download-toggle:active { background: var(--sui-bg-active); }
.mobile-download-panel .entry-download-list {
  padding: 0 max(8px, env(safe-area-inset-right)) 8px max(8px, env(safe-area-inset-left));
}
.mobile-download-panel .entry-download-label { white-space: normal; overflow-wrap: anywhere; }
.mobile-download-panel .entry-download-action { min-width: 48px; min-height: 48px; }
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
