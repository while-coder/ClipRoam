<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, shallowRef } from "vue";
import TextPreview from "./TextPreview.vue";
import { highlightLimit } from "./textLanguages";
import { native } from "../../platform/native";
import { showNotification } from "../../platform/runtime";
import { errorMessage } from "../../utils";

const props = defineProps<{ text: string; fileName?: string }>();
const source = ref(false);
const renderer = shallowRef<typeof import("./markdown")>();
const failed = ref(false);
const tooLarge = computed(() => props.text.length > highlightLimit);
let disposed = false;
onBeforeUnmount(() => { disposed = true; });
onMounted(async () => {
  try {
    const module = await import("./markdown");
    if (!disposed) renderer.value = module;
  } catch {
    if (!disposed) failed.value = true;
  }
});
const html = computed(() => {
  if (source.value || tooLarge.value || !renderer.value) return "";
  return renderer.value.renderMarkdown(props.text);
});
async function openLink(event: MouseEvent): Promise<void> {
  const link = (event.target as Element).closest<HTMLAnchorElement>("a");
  if (!link) return;
  event.preventDefault();
  const href = link.getAttribute("href") ?? "";
  if (!/^(https?:|mailto:)/i.test(href)) return;
  try { await native.openExternalUrl(href); }
  catch (cause) { showNotification({ message: `打开链接失败：${errorMessage(cause)}`, tone: "error" }); }
}
</script>

<template>
  <div class="markdown-preview">
    <slot />
    <div class="markdown-toolbar">
      <span>Markdown</span>
      <SButton :aria-pressed="source" :disabled="tooLarge || failed" @click="source = !source">{{ source ? '预览' : '源码' }}</SButton>
    </div>
    <p v-if="tooLarge || failed" class="markdown-note" role="status">{{ tooLarge ? '内容较大，显示源码。' : 'Markdown 加载失败，显示源码。' }}</p>
    <TextPreview v-if="source || tooLarge || failed" :text="text" :file-name="fileName || 'preview.md'" empty-text="（空文件）" />
    <div v-else-if="!renderer" class="markdown-content markdown-loading" role="status">正在加载 Markdown…</div>
    <article v-else class="markdown-content" tabindex="0" aria-label="Markdown 内容" @click="openLink" v-html="html || '（空文件）'" />
  </div>
</template>

<style scoped>
.markdown-preview { padding: 16px; }
.markdown-toolbar { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 12px; color: var(--color-secondary-text); font-size: 13px; }
.markdown-preview :deep(.text-preview) { padding: 0; }
.markdown-note { margin: 0 0 12px; color: var(--color-secondary-text); font-size: 13px; }
.markdown-content { height: min(510px, calc(100dvh - 290px)); margin: 0; padding: 16px; overflow: auto; overflow-wrap: anywhere; color: var(--color-foreground); background: var(--sui-bg-hover); border: 1px solid var(--sui-border-strong); border-radius: 6px; font-size: 14px; line-height: 1.7; user-select: text; -webkit-user-select: text; }
.markdown-preview :deep(.text-preview-panel) { height: min(510px, calc(100dvh - 290px)); }
.markdown-loading { display: grid; place-items: center; color: var(--color-secondary-text); }
.markdown-content :deep(> :first-child) { margin-top: 0; }
.markdown-content :deep(h1), .markdown-content :deep(h2) { padding-bottom: 8px; border-bottom: 1px solid var(--sui-border-strong); line-height: 1.3; }
.markdown-content :deep(a) { color: var(--sui-primary); text-decoration: underline; }
.markdown-content :deep(blockquote) { margin-inline: 0; padding-left: 16px; border-left: 3px solid var(--sui-border-strong); color: var(--color-secondary-text); }
.markdown-content :deep(pre) { padding: 12px; overflow: auto; border-radius: 6px; background: var(--color-background); }
.markdown-content :deep(code) { font-family: ui-monospace, SFMono-Regular, Consolas, monospace; font-size: 13px; }
.markdown-content :deep(code.hljs) { padding: 0; background: transparent; }
.markdown-content :deep(table) { display: block; max-width: 100%; overflow: auto; border-collapse: collapse; }
.markdown-content :deep(th), .markdown-content :deep(td) { padding: 6px 12px; border: 1px solid var(--sui-border-strong); }
.markdown-content :deep(img) { max-width: 100%; height: auto; }
.markdown-content :deep(hr) { border: 0; border-top: 1px solid var(--sui-border-strong); }
@media (max-width: 640px) { .markdown-preview { padding: 12px; } }
</style>
