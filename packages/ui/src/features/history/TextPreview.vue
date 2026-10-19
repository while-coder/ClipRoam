<script setup lang="ts">
import { Copy } from "lucide-vue-next";
import type { LocalClipboardEntry } from "../../types";

defineProps<{ entry: LocalClipboardEntry; copying: boolean }>();
const emit = defineEmits<{ close: []; copy: [] }>();
</script>

<template>
  <SModal :show="true" title="文本预览" width="760px" @update:show="(value: boolean) => { if (!value) emit('close') }">
    <div class="text-preview">
      <pre class="text-preview-content" tabindex="0" aria-label="文本内容">{{ entry.content }}</pre>
    </div>
    <template #footer>
      <SButton @click="emit('close')">关闭</SButton>
      <SButton type="primary" :disabled="copying" :aria-busy="copying" @click="emit('copy')">
        <span v-if="copying" class="s-spinner" aria-hidden="true" />
        <Copy v-else :size="15" aria-hidden="true" />
        复制全文
      </SButton>
    </template>
  </SModal>
</template>

<style scoped>
.text-preview { padding: 16px; }
.text-preview-content {
  max-height: min(560px, calc(100dvh - 220px));
  margin: 0;
  padding: 12px;
  overflow: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  tab-size: 4;
  color: var(--color-foreground);
  background: var(--sui-bg-hover);
  border: 1px solid var(--sui-border-strong);
  border-radius: 6px;
  font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
  font-size: 13px;
  line-height: 1.6;
  user-select: text;
  -webkit-user-select: text;
}
@media (max-width: 640px) {
  .text-preview { padding: 12px; }
}
</style>
