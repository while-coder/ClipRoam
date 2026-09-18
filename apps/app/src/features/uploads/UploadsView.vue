<script setup lang="ts">
import { Upload } from "lucide-vue-next";
import { computed } from "vue";
import TaskListView, { type TaskRow } from "../../components/TaskListView.vue";
import { percentOf } from "../../utils/format";
import type { ServeTaskSnapshot } from "../sync/fileTransfer";

/**
 * 主窗口侧边栏的「上传」页：收到 file.requested 且本机应答（中继发送）时的
 * 待发送任务台账。台账在 SyncClient 的 FileTransfer 内存中，快照由 App.vue
 * 缓存并透传（事件驱动，无需本组件订阅）。布局骨架在 TaskListView，
 * 这里只注入上传侧的差异项。
 */
const props = defineProps<{
  serveTasks: readonly ServeTaskSnapshot[];
}>();

const activeServeCount = computed(() =>
  props.serveTasks.filter((task) => task.status === "pending" || task.status === "serving").length);

const PROGRESS_STATUSES = ["pending", "serving"];

function statusText(task: TaskRow): string {
  switch (task.status) {
    case "pending": return "待发送";
    case "serving": return "发送中";
    case "succeeded": return "已发送";
    case "skipped": return task.error ? `已跳过·${task.error}` : "已跳过";
    case "failed": return task.error ? `失败·${task.error}` : "失败";
    default: return task.status;
  }
}

function percent(task: TaskRow): number {
  const snapshot = task as ServeTaskSnapshot;
  if (!snapshot.size) return snapshot.status === "succeeded" ? 100 : 0;
  return percentOf(snapshot.sentBytes, snapshot.size);
}
</script>

<template>
  <TaskListView
    title="上传"
    list-label="上传列表"
    :tasks="serveTasks"
    :icon="Upload"
    :status-text="statusText"
    :percent="percent"
    :transferred="(task) => (task as ServeTaskSnapshot).sentBytes"
    spinning-status="serving"
    :progress-statuses="PROGRESS_STATUSES"
    :active-count="activeServeCount"
    empty-text="没有待发送的文件；其他设备请求本机文件时会显示在这里"
  />
</template>
