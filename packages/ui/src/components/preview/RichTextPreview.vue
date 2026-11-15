<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { getAccountSession } from "../../accountSession";
import { errorMessage } from "../../utils";
import type { PreviewFile } from "../../features/history/filePreview";
import { attachmentPath, prepareRtf, renderRtf } from "./rtf";

const props = defineProps<{ entryId: string; file: PreviewFile; files?: PreviewFile[] }>();
const emit = defineEmits<{ ready: [text: string] }>();
const content = ref<HTMLElement>();
const loading = ref(true);
const error = ref("");
const warnings = ref<string[]>([]);
const urls: string[] = [];
const releases: Array<() => void> = [];
let disposed = false;
onBeforeUnmount(() => {
  disposed = true;
  releases.forEach(release => release());
  urls.forEach(url => URL.revokeObjectURL(url));
});

async function imageBlob(bytes: Uint8Array, name: string): Promise<Blob> {
  const extension = name.split(".").pop()?.toLowerCase();
  if (extension === "tif" || extension === "tiff") {
    const { decodeTiff } = await import("./tiff");
    const image = await decodeTiff(bytes);
    const canvas = document.createElement("canvas");
    canvas.width = image.width;
    canvas.height = image.height;
    const context = canvas.getContext("2d");
    if (!context) throw new Error("无法转换 TIFF 图片");
    context.putImageData(new ImageData(image.rgba, image.width, image.height), 0, 0);
    return new Promise((resolve, reject) => canvas.toBlob(blob => blob ? resolve(blob) : reject(new Error("TIFF 转换失败")), "image/png"));
  }
  const types: Record<string, string> = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", bmp: "image/bmp", avif: "image/avif" };
  const type = types[extension ?? ""];
  if (!type) throw new Error("暂不支持此附件格式");
  return new Blob([bytes.slice().buffer], { type });
}

onMounted(async () => {
  const session = getAccountSession();
  if (!session) { error.value = "同步账号未登录"; loading.value = false; return; }
  const active = () => !disposed && !session.signal.aborted;
  const read = async (file: PreviewFile): Promise<Uint8Array> => {
    session.signal.throwIfAborted();
    if (disposed) throw new Error("预览已关闭");
    releases.push(session.files.retain([file.fileId]));
    await session.downloads.ensureLocalFile(props.entryId, file.fileId);
    session.signal.throwIfAborted();
    if (disposed) throw new Error("预览已关闭");
    return session.files.readCached(file.fileId, 0, file.size);
  };
  try {
    if (props.file.size > 5 * 1024 * 1024) throw new Error("富文本正文超过 5 MiB，请下载后查看");
    const prepared = prepareRtf(await read(props.file));
    const rendered = await renderRtf(prepared.bytes);
    if (!active()) return;
    const images = new Map<string, Promise<string>>();
    let totalSize = 0;
    for (const attachment of prepared.attachments) {
      if (!active()) return;
      let replacement: HTMLElement;
      try {
        const path = attachmentPath(attachment.name);
        const file = path && props.files?.find(file => file.path === path);
        if (!file) throw new Error("附件不存在或路径不可用");
        if (!/\.(png|jpe?g|gif|webp|bmp|avif|tiff?)$/i.test(file.name)) throw new Error("暂不支持此附件格式");
        let imageUrl = images.get(file.fileId);
        if (!imageUrl) {
          if (file.size > 20 * 1024 * 1024 || totalSize + file.size > 64 * 1024 * 1024) throw new Error("附件大小超过预览上限");
          totalSize += file.size;
          imageUrl = (async () => {
            const blob = await imageBlob(await read(file), file.name);
            if (!active()) throw new Error("预览已关闭");
            const url = URL.createObjectURL(blob);
            urls.push(url);
            return url;
          })();
          images.set(file.fileId, imageUrl);
        }
        const url = await imageUrl;
        const image = document.createElement("img");
        image.src = url;
        image.alt = attachment.name;
        // Decode now so unavailable image codecs yield a useful placeholder.
        await image.decode();
        replacement = image;
      } catch (cause) {
        replacement = document.createElement("span");
        replacement.textContent = `[附件：${attachment.name}]`;
        replacement.className = "rtf-attachment-placeholder";
        if (active()) warnings.value.push(`${attachment.name}：${errorMessage(cause)}`);
      }
      const walker = document.createTreeWalker(rendered, NodeFilter.SHOW_TEXT);
      while (walker.nextNode()) {
        const node = walker.currentNode as Text;
        const index = node.data.indexOf(attachment.marker);
        if (index < 0) continue;
        const tail = node.splitText(index);
        tail.splitText(attachment.marker.length);
        tail.replaceWith(replacement);
        break;
      }
    }
    if (!active()) return;
    content.value?.replaceChildren(rendered);
    emit("ready", rendered.innerText || rendered.textContent || "");
  } catch (cause) {
    if (active()) error.value = `无法预览富文本：${errorMessage(cause)}`;
  } finally { if (active()) loading.value = false; }
});
</script>

<template>
  <div class="rich-text-preview">
    <p v-if="loading" role="status">正在加载富文本…</p>
    <p v-if="error" role="alert">{{ error }}</p>
    <details v-if="warnings.length" class="rich-text-warnings"><summary>部分附件无法预览（{{ warnings.length }}）</summary><ul><li v-for="(warning, index) in warnings" :key="index">{{ warning }}</li></ul></details>
    <article v-show="!loading && !error" ref="content" class="rich-text-content" tabindex="0" aria-label="富文本文档" />
  </div>
</template>

<style scoped>
.rich-text-preview { display: flex; flex: 1; flex-direction: column; gap: 12px; min-height: 0; padding: 8px; overflow: hidden; }
.rich-text-preview > p { margin: 0; color: var(--sui-fg-muted); }
.rich-text-content { flex: 1; min-height: 0; overflow: auto; overscroll-behavior: contain; padding: 16px; border: 1px solid var(--sui-border); border-radius: 6px; background: white; color: black; overflow-wrap: anywhere; user-select: text; -webkit-user-select: text; }
.rich-text-content :deep(img) { max-width: 100%; height: auto; object-fit: contain; }
.rich-text-content :deep(.rtf-attachment-placeholder) { color: #666; font-style: italic; }
.rich-text-warnings { flex: none; max-height: 100px; overflow: auto; color: var(--sui-fg-muted); font-size: 12px; }
</style>
