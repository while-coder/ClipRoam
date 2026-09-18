<script setup lang="ts">
import { Download } from "lucide-vue-next";
import TaskListView, { type TaskRow } from "../../components/TaskListView.vue";
import { percentOf } from "../../utils/format";
import { activeDownloadCount } from "./useDownloads";
import type { DownloadTaskSnapshot } from "../sync/fileTransfer";

/**
 * 主窗口侧边栏的「下载」页：Rust 全局 Downloader 的任务快照在这里整页展示，
 * 快照由 App.vue 缓存并透传（事件驱动，无需本组件订阅）。布局骨架在
 * TaskListView，这里只注入下载侧的差异项。
 */
defineProps<{
  downloadTasks: readonly DownloadTaskSnapshot[];
}>();

const emit = defineEmits<{
  "cancel-download": [taskId: string];
  "cancel-all-downloads": [];
}>();

const PROGRESS_STATUSES = ["queued", "downloading"];

function statusText(task: TaskRow): string {
  switch (task.status) {
    case "queued": return "排队中";
    case "downloading": return "下载中";
    case "succeeded": return "已完成";
    case "cancelled": return "已取消";
    case "failed": return task.error ? `失败·${task.error}` : "失败";
    default: return task.status;
  }
}

function percent(task: TaskRow): number {
  return percentOf((task as DownloadTaskSnapshot).receivedBytes, task.size);
}
</script>

<template>
  <TaskListView
    title="下载"
    list-label="下载列表"
    :tasks="downloadTasks"
    :icon="Download"
    :status-text="statusText"
    :percent="percent"
    :transferred="(task) => (task as DownloadTaskSnapshot).receivedBytes"
    spinning-status="downloading"
    :progress-statuses="PROGRESS_STATUSES"
    :active-count="activeDownloadCount"
    empty-text="没有下载任务；从其他设备粘贴文件时会显示在这里"
    cancellable
    @cancel-task="emit('cancel-download', $event)"
    @cancel-all="emit('cancel-all-downloads')"
  />
</template>
