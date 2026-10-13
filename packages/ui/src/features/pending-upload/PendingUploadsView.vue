<script setup lang="ts">
import { RotateCcw, Trash2 } from "lucide-vue-next";
import EntryKindIcon from "../../components/EntryKindIcon.vue";
import ClipboardEntryContent from "../../components/ClipboardEntryContent.vue";
import WorkspacePage from "../../components/WorkspacePage.vue";
import { deviceName as deviceDisplayName, isHashing } from "../../utils/entry";
import { percentOf } from "../../utils/format";
import type { Device, LocalClipboardEntry, UploadProgress } from "../../types";

/**
 * 待上传记录成功后离开列表；失败记录显示重试入口，保留原始内容。
 */
const props = defineProps<{
  entries: LocalClipboardEntry[];
  devicesById: Record<string, Device>;
  currentTime: number;
  uploadProgressByEntryId: Record<string, UploadProgress>;
}>();

const emit = defineEmits<{
  remove: [entry: LocalClipboardEntry];
  retry: [entry: LocalClipboardEntry];
}>();

/**
 * 队列行的两个进行中阶段：哈希进度来自 Rust 分批写回的 summary（hashCount
 * 落后于 fileCount 即在算），上传进度由同步客户端按 `p{seq}` 实时上报。
 */
function entryUploadStatus(entry: LocalClipboardEntry): string | undefined {
  if (entry.pendingStatus === "failed") return "同步失败";
  if (isHashing(entry)) return `计算中 ${entry.summary.hashedCount}/${entry.summary.fileCount}`;
  const progress = props.uploadProgressByEntryId[entry.id];
  if (!progress) return undefined;
  return `上传中 ${percentOf(progress.uploadedBytes, progress.totalBytes, 99)}%`;
}
</script>

<template>
  <WorkspacePage title="待上传">
    <template #status>
      <span role="status">共 {{ entries.length }} 条待上传</span>
    </template>

    <section class="history-list" aria-label="待上传列表">
      <div v-for="entry in entries" :key="entry.id" class="history-item pending-item">
        <EntryKindIcon :kind="entry.kind" :root-kind="entry.summary.rootKind" />
        <ClipboardEntryContent :entry="entry" :device-name="deviceDisplayName(devicesById, entry)" :current-time="currentTime">
          <template v-if="entryUploadStatus(entry)">
            <span>·</span>
            <STag :type="entry.pendingStatus === 'failed' ? 'error' : 'info'" size="small">{{ entryUploadStatus(entry) }}</STag>
          </template>
        </ClipboardEntryContent>
        <span class="entry-actions">
          <SIconButton v-if="entry.pendingStatus === 'failed'" :size="28" title="重试上传" aria-label="重试上传" @click.stop="emit('retry', entry)">
            <RotateCcw :size="15" aria-hidden="true" />
          </SIconButton>
          <SIconButton type="error" :size="28" title="删除" aria-label="删除" @click.stop="emit('remove', entry)">
            <Trash2 :size="15" aria-hidden="true" />
          </SIconButton>
        </span>
      </div>

      <SEmpty v-if="!entries.length">没有待上传的内容，所有内容都已同步到服务器</SEmpty>
    </section>
  </WorkspacePage>
</template>
