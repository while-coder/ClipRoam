<script setup lang="ts">
import { ClipboardKind } from "@cliproam/protocol";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { Copy, Download } from "lucide-vue-next";
import { getAccountSession } from "../../accountSession";
import { previewSource, showNotification, usePlatform } from "../../platform/runtime";
import { clipboardPlatform } from "../../platform/clipboard";
import { errorMessage, safeName } from "../../utils";
import { DownloadCancelledError } from "../downloads/downloadManager";
import { audioMimeType, decodePreviewText, filePreviewKind, FilePreviewKind, type PreviewFile } from "./filePreview";
import ImagePreview from "../../components/preview/ImagePreview.vue";
import TextPreview from "../../components/preview/TextPreview.vue";
import MarkdownPreview from "../../components/preview/MarkdownPreview.vue";
import AudioPreview from "../../components/preview/AudioPreview.vue";
import { languageForFile } from "../../components/preview/textLanguages";

const props = defineProps<{ entryId: string; file: PreviewFile; image?: boolean; back?: boolean }>();
const emit = defineEmits<{ close: []; back: [] }>();
const kind = props.image ? ClipboardKind.Image : filePreviewKind(props.file.name);
const loading = ref(false);
const error = ref("");
const text = ref("");
const source = ref("");
const textLimit = 2 * 1024 * 1024;
const truncated = ref(false);
const saving = ref(false);
const copying = ref(false);
const { platformCapabilities } = usePlatform();
const canCopy = computed(() => kind === ClipboardKind.Text || (kind === ClipboardKind.Image
  && platformCapabilities.value.imageClipboard
  && clipboardPlatform.canWriteImage));
let disposed = false;
let audioUrl: string | undefined;
onBeforeUnmount(() => {
  disposed = true;
  if (audioUrl) URL.revokeObjectURL(audioUrl);
});

async function copyToClipboard(): Promise<void> {
  const session = getAccountSession();
  if (!session || copying.value || loading.value || error.value || !canCopy.value) return;
  copying.value = true;
  try {
    // Copy the complete file, even when the text preview only shows its beginning.
    const bytes = await session.files.read(props.file.fileId, 0, props.file.size);
    session.signal.throwIfAborted();
    await clipboardPlatform.write(kind === ClipboardKind.Image
      ? { kind: ClipboardKind.Image, bytes }
      : { kind: ClipboardKind.Text, text: decodePreviewText(bytes) }, false);
    if (!session.signal.aborted) showNotification({ message: "已复制到系统剪贴板", tone: "success" });
  } catch (cause) {
    if (!session.signal.aborted) showNotification({ message: `复制失败：${errorMessage(cause)}`, tone: "error" });
  } finally {
    copying.value = false;
  }
}

async function downloadFile(): Promise<void> {
  const session = getAccountSession();
  if (!session || saving.value) return;
  saving.value = true;
  try {
    const name = props.image ? `${safeName(props.file.name)}.webp` : props.file.name;
    const target = await session.files.selectSaveTarget(true, name);
    if (!target) return;
    session.signal.throwIfAborted();
    await session.downloads.ensureLocalFile(props.entryId, props.file.fileId);
    const entry = await session.history.get(props.entryId);
    // Export this file alone, including when it was opened from a directory.
    const fileEntry = props.image ? entry : {
      ...entry,
      kind: ClipboardKind.Files,
      fileInfo: { [props.file.name]: { f: props.file.fileId, s: props.file.size } },
      imageInfo: undefined,
      sources: undefined,
    };
    await session.files.export(fileEntry, target, session.signal);
    if (!session.signal.aborted) showNotification({ message: "文件已保存", tone: "success" });
  } catch (cause) {
    if (!session.signal.aborted && !(cause instanceof DownloadCancelledError)) {
      showNotification({ message: `下载失败：${errorMessage(cause)}`, tone: "error" });
    }
  } finally {
    saving.value = false;
  }
}

onMounted(async () => {
  if (!kind) return;
  loading.value = true;
  try {
    const session = getAccountSession();
    if (!session) throw new Error("同步账号未登录");
    await session.downloads.ensureLocalFile(props.entryId, props.file.fileId);
    if (disposed) return;
    session.signal.throwIfAborted();
    if (kind === ClipboardKind.Image) {
      const path = await session.files.resolve(props.file.fileId);
      if (!path) throw new Error("图片文件不可用");
      const url = await previewSource(path);
      if (!disposed) source.value = url;
    } else if (kind === FilePreviewKind.Audio) {
      // A typed Blob also works for original files outside the desktop asset scope.
      const bytes = await session.files.read(props.file.fileId, 0, props.file.size);
      if (disposed) return;
      audioUrl = URL.createObjectURL(new Blob([bytes], { type: audioMimeType(props.file.name) }));
      source.value = audioUrl;
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
  <SModal :show="true" :title="file.name" :width="kind === ClipboardKind.Image ? '980px' : '760px'" :body-scrollable="kind !== ClipboardKind.Image" @update:show="(value: boolean) => { if (!value) emit('close') }">
    <div v-if="loading" class="preview-status" role="status"><span class="s-spinner" aria-hidden="true" />正在加载文件…</div>
    <div v-else-if="error" class="preview-status" role="alert">{{ error }}</div>
    <div v-else-if="!kind" class="preview-status">暂不支持此文件类型的预览</div>
    <ImagePreview v-else-if="kind === ClipboardKind.Image && source" :src="source" :title="file.name" />
    <AudioPreview v-else-if="kind === FilePreviewKind.Audio && source" :src="source" :title="file.name" />
    <MarkdownPreview v-else-if="kind === ClipboardKind.Text && languageForFile(file.name) === 'markdown'" :text="text" :file-name="file.name">
      <p v-if="truncated" class="preview-note" role="status">文件较大，仅显示前 2 MiB 内容。</p>
    </MarkdownPreview>
    <TextPreview v-else-if="kind === ClipboardKind.Text" :text="text" :file-name="file.name" label="文件文本内容" empty-text="（空文件）">
      <p v-if="truncated" class="preview-note" role="status">文件较大，仅显示前 2 MiB 内容。</p>
    </TextPreview>
    <template #footer>
      <SButton v-if="back" @click="emit('back')">返回文件列表</SButton>
      <SButton v-if="canCopy" :loading="copying" :disabled="loading || copying || !!error" @click="copyToClipboard">
        <Copy :size="15" aria-hidden="true" />复制到剪贴板
      </SButton>
      <SButton type="primary" :loading="saving" :disabled="loading || saving" @click="downloadFile">
        <Download :size="15" aria-hidden="true" />下载
      </SButton>
      <SButton @click="emit('close')">关闭</SButton>
    </template>
  </SModal>
</template>

<style scoped>
.preview-status { display: flex; align-items: center; justify-content: center; gap: 8px; min-height: 160px; padding: 16px; color: var(--color-secondary-text); }
.preview-note { margin: 0 0 12px; color: var(--color-secondary-text); font-size: 13px; }
</style>
