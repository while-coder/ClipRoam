<script setup lang="ts">
import { LoaderCircle, Upload } from "lucide-vue-next";
import { computed } from "vue";
import { formatFileSize } from "../../utils/format";
import type { ServeTaskSnapshot } from "../sync/fileTransfer";

/**
 * 主窗口侧边栏的「上传」页：收到 file.requested 且本机应答（中继发送）时的
 * 待发送任务台账。台账在 SyncClient 的 FileTransfer 内存中，快照由 App.vue
 * 缓存并透传（事件驱动，无需本组件订阅）。
 */
const props = defineProps<{
  serveTasks: readonly ServeTaskSnapshot[];
}>();

const activeServeCount = computed(() =>
  props.serveTasks.filter((task) => task.status === "pending" || task.status === "serving").length);

function serveTaskStatusText(task: ServeTaskSnapshot): string {
  switch (task.status) {
    case "pending": return "待发送";
    case "serving": return "发送中";
    case "succeeded": return "已发送";
    case "skipped": return task.error ? `已跳过·${task.error}` : "已跳过";
    case "failed": return task.error ? `失败·${task.error}` : "失败";
  }
}

function servePercent(task: ServeTaskSnapshot): number {
  if (!task.size) return task.status === "succeeded" ? 100 : 0;
  return Math.min(100, Math.floor((task.sentBytes / task.size) * 100));
}
</script>

<template>
  <section class="app-content history-content pending-content">
    <header class="titlebar workspace-titlebar">
      <div class="page-title">
        <span>工作区</span>
        <h1>上传</h1>
      </div>
      <div class="titlebar-actions">
        <span class="pending-total" role="status">
          {{ activeServeCount ? `${activeServeCount} 个发送中` : `共 ${serveTasks.length} 个任务` }}
        </span>
      </div>
    </header>

    <section class="history-list downloads-list" aria-label="上传列表">
      <div v-for="task in serveTasks" :key="task.id" class="history-item download-row">
        <span class="kind-icon">
          <LoaderCircle v-if="task.status === 'serving'" :size="18" class="spin" aria-hidden="true" />
          <Upload v-else :size="18" aria-hidden="true" />
        </span>
        <span class="download-info">
          <span class="download-label" :title="task.label">{{ task.label }}</span>
          <span class="download-status" :class="task.status">
            {{ serveTaskStatusText(task) }}
            <template v-if="task.status === 'serving'">· {{ formatFileSize(task.sentBytes) }}/{{ formatFileSize(task.size) }}</template>
            <template v-else-if="task.size">· {{ formatFileSize(task.size) }}</template>
          </span>
          <span v-if="task.status === 'serving' || task.status === 'pending'" class="download-bar">
            <span class="download-bar-fill" :style="{ width: `${servePercent(task)}%` }"></span>
          </span>
        </span>
      </div>

      <div v-if="!serveTasks.length" class="empty-state">
        <span>没有待发送的文件；其他设备请求本机文件时会显示在这里</span>
      </div>
    </section>
  </section>
</template>
