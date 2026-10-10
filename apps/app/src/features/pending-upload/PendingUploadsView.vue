<script setup lang="ts">
import { Monitor, Trash2 } from "lucide-vue-next";
import EntryKindIcon from "../../components/EntryKindIcon.vue";
import { deviceName as deviceDisplayName, isHashing } from "../../utils/entry";
import { formatAge as formatAgeRelative, formatExactDateTime, percentOf } from "../../utils/format";
import type { Device, LocalClipboardEntry, UploadProgress } from "../../types";

/**
 * Everything in this list shares one state — 未同步. Once an entry reaches the
 * server (published, or adopted after a content upload) it disappears from
 * here, so no per-entry status text is rendered.
 */
const props = defineProps<{
  entries: LocalClipboardEntry[];
  devicesById: Record<string, Device>;
  currentTime: number;
  uploadProgressByEntryId: Record<string, UploadProgress>;
}>();

const emit = defineEmits<{
  remove: [entry: LocalClipboardEntry];
}>();

function formatAge(createdAt: string): string {
  return formatAgeRelative(createdAt, props.currentTime);
}

/**
 * 队列行的两个进行中阶段：哈希进度来自 Rust 分批写回的 summary（hashCount
 * 落后于 fileCount 即在算），上传进度由同步客户端按 `p{seq}` 实时上报。
 */
function entryUploadStatus(entry: LocalClipboardEntry): string | undefined {
  if (isHashing(entry)) return `计算中 ${entry.summary.hashedCount}/${entry.summary.fileCount}`;
  const progress = props.uploadProgressByEntryId[entry.id];
  if (!progress) return undefined;
  return `上传中 ${percentOf(progress.uploadedBytes, progress.totalBytes, 99)}%`;
}
</script>

<template>
  <section class="app-content history-content pending-content">
    <header class="titlebar workspace-titlebar">
      <div class="page-title">
        <span>工作区</span>
        <h1>待上传</h1>
      </div>
      <div class="titlebar-actions">
        <span class="pending-total" role="status">共 {{ entries.length }} 条待上传</span>
      </div>
    </header>

    <section class="history-list" aria-label="待上传列表">
      <div v-for="entry in entries" :key="entry.id" class="history-item pending-item">
        <EntryKindIcon :kind="entry.kind" :root-kind="entry.summary.rootKind" />
        <span class="entry-body">
          <span class="entry-content">{{ entry.content }}</span>
          <span class="entry-meta">
            <Monitor :size="12" /> {{ deviceDisplayName(props.devicesById, entry) }}
            <span>·</span>
            <span :title="formatExactDateTime(entry.createdAt)">{{ formatAge(entry.createdAt) }}</span>
            <template v-if="entryUploadStatus(entry)">
              <span>·</span>
              <STag type="info" size="small">{{ entryUploadStatus(entry) }}</STag>
            </template>
          </span>
        </span>
        <span class="entry-actions">
          <SIconButton type="error" :size="28" title="删除" aria-label="删除" @click.stop="emit('remove', entry)">
            <Trash2 :size="15" aria-hidden="true" />
          </SIconButton>
        </span>
      </div>

      <SEmpty v-if="!entries.length">没有待上传的内容，所有内容都已同步到服务器</SEmpty>
    </section>
  </section>
</template>
