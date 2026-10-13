<script setup lang="ts">
import { Upload } from "lucide-vue-next";
import { computed } from "vue";
import WorkspacePage from "../../components/WorkspacePage.vue";
import WorkspaceMenu from "../../components/WorkspaceMenu.vue";
import TransferProgress from "../../components/TransferProgress.vue";
import { formatFileSize, percentOf } from "../../utils/format";
import type { ServeTaskSnapshot } from "./uploadManager";

/** 中继上传台账由账号的 UploadManager 管理，App.vue 将快照传给本页。 */
const props = defineProps<{
  serveTasks: readonly ServeTaskSnapshot[];
}>();

const activeServeCount = computed(() =>
  props.serveTasks.filter((task) => task.status === "pending" || task.status === "serving").length);

function statusText(task: ServeTaskSnapshot): string {
  switch (task.status) {
    case "pending": return "待发送";
    case "serving": return "发送中";
    case "succeeded": return "已发送";
    case "skipped": return task.error ? `已跳过·${task.error}` : "已跳过";
    case "failed": return task.error ? `失败·${task.error}` : "失败";
    default: return task.status;
  }
}

function percent(task: ServeTaskSnapshot): number {
  if (!task.size) return task.status === "succeeded" ? 100 : 0;
  return percentOf(task.sentBytes, task.size);
}
</script>

<template>
  <WorkspacePage title="上传">
    <template #menu><WorkspaceMenu /></template>
    <template #status>
      <span role="status">{{ activeServeCount ? `${activeServeCount} 个进行中` : `共 ${serveTasks.length} 个任务` }}</span>
    </template>

    <section class="history-list downloads-list" aria-label="上传列表">
      <div v-for="task in serveTasks" :key="task.id" class="history-item download-row">
        <span class="kind-icon">
          <span v-if="task.status === 'serving'" class="s-spinner" aria-hidden="true" />
          <Upload v-else :size="18" aria-hidden="true" />
        </span>
        <span class="download-info">
          <span class="download-label" :title="task.label">{{ task.label }}</span>
          <span class="download-status" :class="task.status">
            {{ statusText(task) }}
            <template v-if="task.status === 'serving'">· {{ formatFileSize(task.sentBytes) }}/{{ formatFileSize(task.size) }}</template>
            <template v-else-if="task.size">· {{ formatFileSize(task.size) }}</template>
          </span>
          <TransferProgress v-if="task.status === 'pending' || task.status === 'serving'" :percent="percent(task)" :label="`${task.label} 上传进度`" />
        </span>
      </div>

      <SEmpty v-if="!serveTasks.length">没有待发送的文件；其他设备请求本机文件时会显示在这里</SEmpty>
    </section>
  </WorkspacePage>
</template>
