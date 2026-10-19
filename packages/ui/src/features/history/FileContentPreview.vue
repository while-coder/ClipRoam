<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { getAccountSession } from "../../accountSession";
import { previewSource } from "../../platform/runtime";
import { errorMessage } from "../../utils";
import { decodePreviewText, filePreviewKind, type PreviewFile } from "./filePreview";
import ImagePreview from "./ImagePreview.vue";

const props = defineProps<{ entryId: string; file: PreviewFile; image?: boolean; back?: boolean }>();
const emit = defineEmits<{ close: []; back: [] }>();
const kind = props.image ? "image" : filePreviewKind(props.file.name);
const loading = ref(false);
const error = ref("");
const text = ref("");
const source = ref("");
const textLimit = 2 * 1024 * 1024;
const truncated = ref(false);
let disposed = false;
onBeforeUnmount(() => { disposed = true; });

onMounted(async () => {
  if (!kind) return;
  loading.value = true;
  try {
    const session = getAccountSession();
    if (!session) throw new Error("同步账号未登录");
    await session.downloads.ensureLocalFile(props.entryId, props.file.fileId);
    if (disposed) return;
    session.signal.throwIfAborted();
    if (kind === "image") {
      const path = await session.files.resolve(props.file.fileId);
      if (!path) throw new Error("图片文件不可用");
      const url = await previewSource(path);
      if (!disposed) source.value = url;
    } else {
      const bytes = await session.files.read(props.file.fileId, 0, Math.min(props.file.size, textLimit));
      if (disposed) return;
      truncated.value = props.file.size > textLimit;
      text.value = decodePreviewText(bytes, truncated.value);
    }
  } catch (cause) {
    if (!disposed) error.value = `无法预览文件：${errorMessage(cause)}`;
  } finally {
    if (!disposed) loading.value = false;
  }
});
</script>

<template>
  <SModal :show="true" :title="file.name" :width="kind === 'image' ? '980px' : '760px'" @update:show="(value: boolean) => { if (!value) emit('close') }">
    <div v-if="loading" class="preview-status" role="status"><span class="s-spinner" aria-hidden="true" />正在加载文件…</div>
    <div v-else-if="error" class="preview-status" role="alert">{{ error }}</div>
    <div v-else-if="!kind" class="preview-status">暂不支持此文件类型的预览</div>
    <ImagePreview v-else-if="kind === 'image' && source" :src="source" :title="file.name" />
    <div v-else-if="kind === 'text'" class="text-preview">
      <p v-if="truncated" role="status">文件较大，仅显示前 2 MiB 内容。</p>
      <pre tabindex="0" aria-label="文件文本内容">{{ text || '（空文件）' }}</pre>
    </div>
    <template #footer>
      <SButton v-if="back" @click="emit('back')">返回文件列表</SButton>
      <SButton @click="emit('close')">关闭</SButton>
    </template>
  </SModal>
</template>

<style scoped>
.preview-status { display: flex; align-items: center; justify-content: center; gap: 8px; min-height: 160px; padding: 16px; color: var(--color-secondary-text); }
.text-preview { padding: 16px; }
.text-preview p { margin: 0 0 12px; color: var(--color-secondary-text); font-size: 13px; }
.text-preview pre { max-height: min(560px, calc(100dvh - 240px)); margin: 0; padding: 12px; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; tab-size: 4; color: var(--color-foreground); background: var(--sui-bg-hover); border: 1px solid var(--sui-border-strong); border-radius: 6px; font: 13px/1.6 ui-monospace, SFMono-Regular, Consolas, monospace; user-select: text; -webkit-user-select: text; }
@media (max-width: 640px) { .text-preview { padding: 12px; } }
</style>
