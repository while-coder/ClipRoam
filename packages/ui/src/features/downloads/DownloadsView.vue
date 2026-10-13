<script setup lang="ts">
import { computed, ref } from "vue";
import { ChevronDown, Download, Trash2, X } from "lucide-vue-next";
import WorkspacePage from "../../components/WorkspacePage.vue";
import WorkspaceMenu from "../../components/WorkspaceMenu.vue";
import TransferProgress from "../../components/TransferProgress.vue";
import { formatExactDateTime, formatFileSize, percentOf } from "../../utils/format";
import type { DownloadRecord, DownloadTaskSnapshot } from "./types";

const props = defineProps<{ records: readonly DownloadRecord[] }>();
const emit = defineEmits<{
  "cancel-record": [recordId: string];
  "cancel-all": [];
  "remove-record": [recordId: string];
  "clear-finished": [];
}>();
const expandedRecords = ref(new Set<string>());
const timeFormatter = new Intl.DateTimeFormat("zh-CN", {
  month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false,
});
function toggleDetails(id: string): void {
  if (expandedRecords.value.has(id)) expandedRecords.value.delete(id);
  else expandedRecords.value.add(id);
}
const activeCount = computed(() => props.records.filter((record) => record.finishedMs === null).length);
const hasFinished = computed(() => props.records.some((record) => record.finishedMs !== null));
const canCancel = computed(() => props.records.some((record) => record.status === "queued" || record.status === "downloading"));
const labels: Record<DownloadRecord["status"], string> = {
  queued: "排队中", downloading: "下载中", saving: "正在保存", succeeded: "已完成",
  failed: "失败", cancelled: "已取消", interrupted: "已中断",
};
function taskStatus(file: DownloadTaskSnapshot): string { return labels[file.status]; }
function percent(record: DownloadRecord): number {
  return record.status === "saving" ? 100 : percentOf(record.receivedBytes, record.size, record.finishedMs === null ? 99 : 100);
}
</script>

<template>
  <WorkspacePage title="下载">
    <template #menu><WorkspaceMenu /></template>
    <template #status>
      <span role="status">{{ activeCount ? `${activeCount} 个进行中` : `共 ${records.length} 条记录` }}</span>
    </template>
    <template #actions>
      <SButton v-if="canCancel" type="error" @click="emit('cancel-all')">全部取消</SButton>
      <SButton :disabled="!hasFinished" @click="emit('clear-finished')">
        <Trash2 :size="15" aria-hidden="true" />
        清空已结束
      </SButton>
    </template>
    <section class="history-list downloads-list" aria-label="下载记录">
      <article v-for="record in records" :key="record.id" class="download-record">
        <div class="download-record-main">
          <button class="download-record-summary" type="button"
            :aria-expanded="expandedRecords.has(record.id)" :aria-controls="`download-details-${record.id}`"
            @click="toggleDetails(record.id)">
            <span class="kind-icon">
              <span v-if="record.status === 'downloading' || record.status === 'saving'" class="s-spinner" aria-hidden="true" />
              <Download v-else :size="18" aria-hidden="true" />
            </span>
            <span class="download-info">
              <span class="download-record-heading">
                <span class="download-label" :title="record.label">{{ record.label }}</span>
                <ChevronDown :size="14" class="download-record-chevron" :class="{ expanded: expandedRecords.has(record.id) }" aria-hidden="true" />
              </span>
              <span class="download-record-meta">
                <span class="download-status" :class="record.status">{{ labels[record.status] }}</span>
                <span v-if="record.fileCount > 1">· {{ record.fileCount }} 个文件</span>
                <span>· <template v-if="record.finishedMs === null">{{ formatFileSize(record.receivedBytes) }}/</template>{{ formatFileSize(record.size) }}</span>
                <time :datetime="new Date(record.createdMs).toISOString()" :title="formatExactDateTime(new Date(record.createdMs).toISOString())">· {{ timeFormatter.format(record.createdMs) }}</time>
              </span>
              <span v-if="record.error" class="download-record-error download-record-error-summary" :title="record.error">{{ record.error }}</span>
              <TransferProgress v-if="record.finishedMs === null" :percent="percent(record)" :label="`${record.label} 下载进度`" />
            </span>
          </button>
          <button v-if="record.status === 'queued' || record.status === 'downloading'" type="button"
            class="download-record-icon-button" :aria-label="`取消下载 ${record.label}`" title="取消下载" @click="emit('cancel-record', record.id)">
            <X :size="18" aria-hidden="true" />
          </button>
          <button v-else-if="record.finishedMs !== null" type="button"
            class="download-record-icon-button" :aria-label="`删除下载记录 ${record.label}`" title="删除记录，不删除文件" @click="emit('remove-record', record.id)">
            <Trash2 :size="17" aria-hidden="true" />
          </button>
        </div>
        <div v-if="expandedRecords.has(record.id)" :id="`download-details-${record.id}`" class="download-record-details">
          <span class="download-record-destination">保存位置：{{ record.destination }}</span>
          <span v-if="record.error" class="download-record-error">{{ record.error }}</span>
          <div v-for="file in record.fileCount > 1 ? record.files : []" :key="file.id" class="download-record-file">
            <span>{{ file.label }}</span>
            <span class="download-status" :class="file.status">{{ taskStatus(file) }} · {{ formatFileSize(file.receivedBytes) }}/{{ formatFileSize(file.size) }}</span>
            <span v-if="file.error" class="download-record-error">{{ file.error }}</span>
          </div>
        </div>
      </article>
      <SEmpty v-if="!records.length">没有下载记录；下载图片或文件后会显示在这里</SEmpty>
    </section>
    <template #footer><span title="保留最近 30 天，最多 500 条已结束记录；清理记录不会删除文件">近 30 天 · 最多 500 条 · 清理记录不删除文件</span></template>
  </WorkspacePage>
</template>

<style scoped>
.download-record { padding: 6px 8px; border-bottom: 1px solid var(--border); }
.download-record-main { display: grid; grid-template-columns: minmax(0, 1fr) 44px; align-items: center; gap: 8px; }
.download-record-summary { display: grid; grid-template-columns: 32px minmax(0, 1fr); align-items: center; gap: 8px; width: 100%; min-height: 44px; padding: 0; border: 0; border-radius: 6px; background: transparent; color: var(--color-foreground); font: inherit; text-align: left; cursor: pointer; touch-action: manipulation; }
.download-record-summary:hover, .download-record-icon-button:hover { background: var(--sui-bg-hover); }
.download-record-summary:active, .download-record-icon-button:active { background: var(--sui-bg-active); }
.download-record-summary:focus-visible, .download-record-icon-button:focus-visible { outline: 2px solid var(--color-accent); outline-offset: 2px; }
.download-info { display: grid; min-width: 0; gap: 3px; }
.download-record-heading { display: flex; align-items: center; gap: 6px; min-width: 0; }
.download-record-chevron { flex-shrink: 0; color: var(--color-secondary-text); }
.download-record-chevron.expanded { transform: rotate(180deg); }
.download-record-meta { display: flex; flex-wrap: wrap; gap: 0 4px; color: var(--color-secondary-text); font-size: 11px; line-height: 1.5; }
.download-record-meta > * { white-space: nowrap; }
.download-record-error { color: var(--sui-error); font-size: 12px; overflow-wrap: anywhere; }
.download-record-error-summary { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.download-record-icon-button { display: grid; place-items: center; width: 44px; height: 44px; padding: 0; border: 0; border-radius: 8px; background: transparent; color: var(--color-secondary-text); cursor: pointer; touch-action: manipulation; }
.download-record-icon-button:hover { color: var(--sui-error); }
.download-record-details { display: grid; gap: 4px; margin: 4px 44px 6px 40px; color: var(--color-secondary-text); font-size: 12px; overflow-wrap: anywhere; }
.download-record-file { display: grid; gap: 3px; padding: 6px 0; }
@media (max-width: 640px) {
  .download-record-summary { grid-template-columns: 44px minmax(0, 1fr); }
  .download-record-details { margin-left: 52px; }
}
</style>
