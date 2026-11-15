<script setup lang="ts">
import { ref } from "vue";
import { Copy, Download } from "lucide-vue-next";
import { ClipboardKind } from "@cliproam/protocol";
import { getAccountSession } from "../../accountSession";
import { clipboardPlatform } from "../../platform/clipboard";
import { showNotification } from "../../platform/runtime";
import { errorMessage } from "../../utils";
import { DownloadCancelledError } from "../../features/downloads/downloadManager";
import type { RtfdDocument } from "../../features/history/filePreview";
import RichTextPreview from "./RichTextPreview.vue";

const props = defineProps<{ entryId: string; document: RtfdDocument; back?: boolean }>();
const emit = defineEmits<{ close: []; back: [] }>();
const text = ref("");
const ready = ref(false);
const copying = ref(false);
const downloading = ref(false);
function onReady(value: string): void { text.value = value; ready.value = true; }
function close(): void { if (props.back) emit("back"); else emit("close"); }

async function copyText(): Promise<void> {
  const session = getAccountSession();
  if (!session || !ready.value || copying.value) return;
  copying.value = true;
  try {
    await clipboardPlatform.write({ kind: ClipboardKind.Text, text: text.value }, false);
    session.signal.throwIfAborted();
    showNotification({ message: "已复制到系统剪贴板", tone: "success" });
  } catch (cause) {
    if (!session.signal.aborted) showNotification({ message: `复制失败：${errorMessage(cause)}`, tone: "error" });
  } finally { copying.value = false; }
}

async function downloadPackage(): Promise<void> {
  const session = getAccountSession();
  if (!session || downloading.value) return;
  downloading.value = true;
  const release = session.files.retain(props.document.files.map(file => file.fileId));
  try {
    const target = await session.files.selectSaveTarget(false, props.document.name);
    if (!target) return;
    session.signal.throwIfAborted();
    showNotification({ message: "已添加到下载列表", tone: "success" });
    for (const file of props.document.files) {
      await session.downloads.ensureLocalFile(props.entryId, file.fileId);
    }
    const entry = await session.history.get(props.entryId);
    await session.files.export({
      ...entry,
      kind: ClipboardKind.Files,
      fileInfo: { [props.document.name]: props.document.tree },
      imageInfo: undefined,
      sources: undefined,
    }, target, session.signal);
    session.signal.throwIfAborted();
    showNotification({ message: "文件已保存", tone: "success" });
  } catch (cause) {
    if (!session.signal.aborted && !(cause instanceof DownloadCancelledError)) {
      showNotification({ message: `下载失败：${errorMessage(cause)}`, tone: "error" });
    }
  } finally { release(); downloading.value = false; }
}
</script>

<template>
  <SModal class="rtfd-preview-modal entry-preview-modal" fill-height :show="true" :title="document.name"
    width="760px" :body-scrollable="false" :mask-closable="true" @update:show="(value: boolean) => { if (!value) close(); }">
    <RichTextPreview :entry-id="entryId" :file="document.body" :files="document.files" @ready="onReady" />
    <template #footer>
      <SButton v-if="ready" :loading="copying" :disabled="copying" @click="copyText">
        <Copy :size="15" aria-hidden="true" />复制到剪贴板
      </SButton>
      <SButton type="primary" :loading="downloading" :disabled="downloading" @click="downloadPackage">
        <Download :size="15" aria-hidden="true" />下载
      </SButton>
    </template>
  </SModal>
</template>
