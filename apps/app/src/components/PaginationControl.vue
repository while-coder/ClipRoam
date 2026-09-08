<script setup lang="ts">
import { ChevronLeft, ChevronRight } from "lucide-vue-next";

defineProps<{
  page: number;
  pageCount: number;
}>();

const emit = defineEmits<{
  "update:page": [page: number];
}>();
</script>

<template>
  <nav class="pagination-control" aria-label="历史记录分页">
    <button
      class="pagination-button"
      type="button"
      title="上一页"
      aria-label="上一页"
      :disabled="page <= 1"
      @click="emit('update:page', page - 1)"
    >
      <ChevronLeft :size="15" aria-hidden="true" />
      <span>上一页</span>
    </button>
    <span class="pagination-status" aria-live="polite">第 {{ page }} / {{ pageCount }} 页</span>
    <button
      class="pagination-button"
      type="button"
      title="下一页"
      aria-label="下一页"
      :disabled="page >= pageCount"
      @click="emit('update:page', page + 1)"
    >
      <span>下一页</span>
      <ChevronRight :size="15" aria-hidden="true" />
    </button>
  </nav>
</template>
