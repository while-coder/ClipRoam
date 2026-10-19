<script setup lang="ts">
withDefaults(defineProps<{
  text: string;
  label?: string;
  emptyText?: string;
}>(), {
  label: "文本内容",
  emptyText: "",
});
</script>

<template>
  <div class="text-preview">
    <slot />
    <pre class="text-preview-content" tabindex="0" :aria-label="label">{{ text || emptyText }}</pre>
  </div>
</template>

<style scoped>
.text-preview { padding: 16px; }
.text-preview-content {
  height: min(560px, calc(100dvh - 240px));
  box-sizing: border-box;
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
