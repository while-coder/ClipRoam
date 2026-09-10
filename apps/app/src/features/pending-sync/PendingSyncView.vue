<script setup lang="ts">
import {
  Clipboard,
  File,
  FileText,
  FolderOpen,
  Image,
  Monitor,
  Trash2,
} from "lucide-vue-next";
import { deviceName as deviceDisplayName } from "../../utils/entry";
import { formatAge as formatAgeRelative, formatExactDateTime } from "../../utils/format";
import type { ClipboardEntry, Device, LocalClipboardEntry } from "../../types";

/**
 * Everything in this list shares one state — 未同步. Once an entry reaches the
 * server (published, or adopted after a content upload) it disappears from
 * here, so no per-entry status text is rendered.
 */
const props = defineProps<{
  entries: LocalClipboardEntry[];
  devicesById: Record<string, Device>;
  currentTime: number;
}>();

const emit = defineEmits<{
  remove: [entry: ClipboardEntry];
}>();

function formatAge(createdAt: string): string {
  return formatAgeRelative(createdAt, props.currentTime);
}
</script>

<template>
  <section class="app-content history-content pending-content">
    <header class="titlebar workspace-titlebar">
      <div class="page-title">
        <span>工作区</span>
        <h1>待同步</h1>
      </div>
      <div class="titlebar-actions">
        <span class="pending-total" role="status">共 {{ entries.length }} 条待同步</span>
      </div>
    </header>

    <section class="history-list" aria-label="待同步列表">
      <div v-for="entry in entries" :key="entry.id" class="history-item pending-item">
        <span class="kind-icon">
          <FileText v-if="entry.kind === 'text'" :size="18" />
          <File v-else-if="entry.kind === 'files' && entry.summary.rootKind === 'file'" :size="18" />
          <FolderOpen v-else-if="entry.kind === 'files'" :size="18" />
          <Image v-else-if="entry.kind === 'image'" :size="18" />
          <Clipboard v-else :size="18" />
        </span>
        <span class="entry-body">
          <span class="entry-content">{{ entry.content }}</span>
          <span class="entry-meta">
            <Monitor :size="12" /> {{ deviceDisplayName(props.devicesById, entry) }}
            <span>·</span>
            <span :title="formatExactDateTime(entry.createdAt)">{{ formatAge(entry.createdAt) }}</span>
          </span>
        </span>
        <span class="entry-actions">
          <span
            class="item-action danger"
            role="button"
            tabindex="0"
            title="删除"
            aria-label="删除"
            @click.stop="emit('remove', entry)"
            @keydown.enter.stop="emit('remove', entry)"
          ><Trash2 :size="15" /></span>
        </span>
      </div>

      <div v-if="!entries.length" class="empty-state">
        <span>没有待同步的内容，所有内容都已同步到服务器</span>
      </div>
    </section>
  </section>
</template>
