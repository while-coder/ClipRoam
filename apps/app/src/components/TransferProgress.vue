<script setup lang="ts">
import { computed } from "vue";

const props = defineProps<{ percent: number; label: string }>();
const value = computed(() => Number.isFinite(props.percent) ? Math.min(100, Math.max(0, props.percent)) : 0);
</script>

<template>
  <div class="download-bar" role="progressbar" :aria-label="label" :aria-valuenow="value" :aria-valuemin="0" :aria-valuemax="100">
    <span class="download-bar-fill" :style="{ width: `${value}%` }"></span>
  </div>
</template>
