<script setup lang="ts">
import { Clipboard, File, FileText, FolderOpen, Image, LoaderCircle } from "lucide-vue-next";
import type { LocalClipboardEntry } from "../../types";

/**
 * 条目的 kind 图标（HistoryView / PendingSyncView 共用）；激活或哈希进行中
 * 时转圈。外壳 class 沿用全局 .kind-icon（styles.css）。
 */
defineProps<{
  kind: LocalClipboardEntry["kind"];
  rootKind?: string;
  loading?: boolean;
}>();
</script>

<template>
  <span class="kind-icon">
    <LoaderCircle v-if="loading" :size="18" class="spin" />
    <FileText v-else-if="kind === 'text'" :size="18" />
    <File v-else-if="kind === 'files' && rootKind === 'file'" :size="18" />
    <FolderOpen v-else-if="kind === 'files'" :size="18" />
    <Image v-else-if="kind === 'image'" :size="18" />
    <Clipboard v-else :size="18" />
  </span>
</template>
