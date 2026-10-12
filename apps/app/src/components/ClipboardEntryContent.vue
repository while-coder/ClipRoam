<script setup lang="ts">
import { Monitor } from "lucide-vue-next";
import { formatAge, formatExactDateTime } from "../utils/format";
import type { ClipboardEntry } from "../types";

defineProps<{
  entry: Pick<ClipboardEntry, "content" | "createdAt">;
  deviceName: string;
  currentTime: number;
}>();
</script>

<template>
  <span class="entry-body">
    <span class="entry-content">{{ entry.content }}</span>
    <span class="entry-meta">
      <Monitor :size="12" aria-hidden="true" /> {{ deviceName }}
      <span>·</span>
      <span :title="formatExactDateTime(entry.createdAt)">{{ formatAge(entry.createdAt, currentTime) }}</span>
      <slot />
    </span>
  </span>
</template>
