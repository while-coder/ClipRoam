<script setup lang="ts">
import { LoaderCircle, X } from "lucide-vue-next";
import type { Component } from "vue";
import { formatFileSize } from "../utils/format";

/**
 * 传输任务列表的通用骨架：上传/下载两页共用（头部、任务行、进度条、空态）。
 * 任务类型差异（图标、状态文案、进度、可否取消）由调用方以 props 注入。
 */
export interface TaskRow {
  id: string;
  label: string;
  status: string;
  size: number;
  /** 终态补充说明（跳过原因 / 失败原因）。 */
  error?: string;
}

const props = defineProps<{
  /** 页标题（也是取消按钮文案的一部分）。 */
  title: string;
  listLabel: string;
  tasks: readonly TaskRow[];
  /** 空闲态图标；活动态统一用 LoaderCircle 旋转。 */
  icon: Component;
  statusText: (task: TaskRow) => string;
  percent: (task: TaskRow) => number;
  /** 活动态已传输字节数（receivedBytes / sentBytes）。 */
  transferred: (task: TaskRow) => number;
  /** 正在传输的状态名（旋转图标 + 实时字节文案）。 */
  spinningStatus: string;
  /** 显示进度条与取消按钮的状态名。 */
  progressStatuses: readonly string[];
  activeCount: number;
  emptyText: string;
  cancellable?: boolean;
}>();

const emit = defineEmits<{
  "cancel-task": [taskId: string];
  "cancel-all": [];
}>();

function inProgress(task: TaskRow): boolean {
  return props.progressStatuses.includes(task.status);
}
</script>

<template>
  <section class="app-content history-content pending-content">
    <header class="titlebar workspace-titlebar">
      <div class="page-title">
        <span>工作区</span>
        <h1>{{ title }}</h1>
      </div>
      <div class="titlebar-actions">
        <button
          v-if="cancellable && activeCount"
          class="downloads-cancel-all"
          type="button"
          @click="emit('cancel-all')"
        >
          全部取消
        </button>
        <span class="pending-total" role="status">
          {{ activeCount ? `${activeCount} 个进行中` : `共 ${tasks.length} 个任务` }}
        </span>
      </div>
    </header>

    <section class="history-list downloads-list" :aria-label="listLabel">
      <div v-for="task in tasks" :key="task.id" class="history-item download-row">
        <span class="kind-icon">
          <LoaderCircle v-if="task.status === spinningStatus" :size="18" class="spin" aria-hidden="true" />
          <component :is="icon" v-else :size="18" aria-hidden="true" />
        </span>
        <span class="download-info">
          <span class="download-label" :title="task.label">{{ task.label }}</span>
          <span class="download-status" :class="task.status">
            {{ statusText(task) }}
            <template v-if="task.status === spinningStatus">· {{ formatFileSize(transferred(task)) }}/{{ formatFileSize(task.size) }}</template>
            <template v-else-if="task.size">· {{ formatFileSize(task.size) }}</template>
          </span>
          <span v-if="inProgress(task)" class="download-bar">
            <span class="download-bar-fill" :style="{ width: `${percent(task)}%` }"></span>
          </span>
        </span>
        <span v-if="cancellable" class="entry-actions">
          <button
            v-if="inProgress(task)"
            class="item-action danger download-cancel"
            type="button"
            :title="`取消${title}`"
            :aria-label="`取消${title}`"
            @click="emit('cancel-task', task.id)"
          >
            <X :size="15" />
          </button>
        </span>
      </div>

      <div v-if="!tasks.length" class="empty-state">
        <span>{{ emptyText }}</span>
      </div>
    </section>
  </section>
</template>
