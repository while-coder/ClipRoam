<script setup lang="ts">
import { Download, LoaderCircle, X } from "lucide-vue-next";
import { formatFileSize, percentOf } from "../../utils/format";
import { activeDownloadCount } from "./useDownloads";
import type { DownloadTaskSnapshot } from "../sync/fileTransfer";

/**
 * 主窗口侧边栏的「下载」页：Rust 全局 Downloader 的任务快照在这里整页展示，
 * 快照由 App.vue 缓存并透传（事件驱动，无需本组件订阅）。
 */
defineProps<{
  downloadTasks: readonly DownloadTaskSnapshot[];
}>();

const emit = defineEmits<{
  "cancel-download": [taskId: string];
  "cancel-all-downloads": [];
}>();

function downloadTaskStatusText(task: DownloadTaskSnapshot): string {
  switch (task.status) {
    case "queued": return "排队中";
    case "downloading": return "下载中";
    case "succeeded": return "已完成";
    case "cancelled": return "已取消";
    case "failed": return task.error ? `失败·${task.error}` : "失败";
  }
}

function downloadPercent(task: DownloadTaskSnapshot): number {
  return percentOf(task.receivedBytes, task.size);
}
</script>

<template>
  <section class="app-content history-content pending-content">
    <header class="titlebar workspace-titlebar">
      <div class="page-title">
        <span>工作区</span>
        <h1>下载</h1>
      </div>
      <div class="titlebar-actions">
        <button
          v-if="activeDownloadCount"
          class="downloads-cancel-all"
          type="button"
          @click="emit('cancel-all-downloads')"
        >
          全部取消
        </button>
        <span class="pending-total" role="status">
          {{ activeDownloadCount ? `${activeDownloadCount} 个进行中` : `共 ${downloadTasks.length} 个任务` }}
        </span>
      </div>
    </header>

    <section class="history-list downloads-list" aria-label="下载列表">
      <div v-for="task in downloadTasks" :key="task.id" class="history-item download-row">
        <span class="kind-icon">
          <LoaderCircle v-if="task.status === 'downloading'" :size="18" class="spin" aria-hidden="true" />
          <Download v-else :size="18" aria-hidden="true" />
        </span>
        <span class="download-info">
          <span class="download-label" :title="task.label">{{ task.label }}</span>
          <span class="download-status" :class="task.status">
            {{ downloadTaskStatusText(task) }}
            <template v-if="task.status === 'downloading'">· {{ formatFileSize(task.receivedBytes) }}/{{ formatFileSize(task.size) }}</template>
            <template v-else-if="task.size">· {{ formatFileSize(task.size) }}</template>
          </span>
          <span v-if="task.status === 'downloading' || task.status === 'queued'" class="download-bar">
            <span class="download-bar-fill" :style="{ width: `${downloadPercent(task)}%` }"></span>
          </span>
        </span>
        <span class="entry-actions">
          <button
            v-if="task.status === 'queued' || task.status === 'downloading'"
            class="item-action danger download-cancel"
            type="button"
            title="取消下载"
            aria-label="取消下载"
            @click="emit('cancel-download', task.id)"
          >
            <X :size="15" />
          </button>
        </span>
      </div>

      <div v-if="!downloadTasks.length" class="empty-state">
        <span>没有下载任务；从其他设备粘贴文件时会显示在这里</span>
      </div>
    </section>
  </section>
</template>
