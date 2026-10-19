<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from "vue";
import { highlightLimit, languageForFile, textLanguages } from "./textLanguages";

const props = withDefaults(defineProps<{
  text: string;
  fileName?: string;
  label?: string;
  emptyText?: string;
}>(), {
  label: "文本内容",
  emptyText: "",
});
const language = ref("auto");
const highlighter = shallowRef<typeof import("./textHighlight")>();
const loadFailed = ref(false);
let disposed = false;
onBeforeUnmount(() => { disposed = true; });
onMounted(async () => {
  try {
    const module = await import("./textHighlight");
    if (!disposed) highlighter.value = module;
  } catch {
    if (!disposed) loadFailed.value = true;
  }
});
watch(() => props.fileName, () => { language.value = "auto"; });
const selectedLanguage = computed(() => language.value === "auto"
  ? languageForFile(props.fileName) ?? "auto" : language.value);
const highlighted = computed(() => highlighter.value?.highlightText(props.text, selectedLanguage.value));
const detectedLabel = computed(() => {
  const detected = highlighted.value?.language;
  if (!detected || detected === "plaintext") return "纯文本";
  return textLanguages.find(([value]) => value === detected)?.[1] ?? detected;
});
</script>

<template>
  <div class="text-preview">
    <slot />
    <div class="text-preview-panel">
      <div class="text-preview-toolbar">
        <label>语言
          <select v-model="language">
            <option value="auto">自动识别</option>
            <option value="plaintext">纯文本</option>
            <option v-for="[value, name] in textLanguages" :key="value" :value="value">{{ name }}</option>
          </select>
        </label>
        <span role="status">{{ text.length > highlightLimit ? '内容较大，显示纯文本' : loadFailed ? '高亮加载失败，显示纯文本' : highlighter ? detectedLabel : '正在加载高亮…' }}</span>
      </div>
      <pre class="text-preview-content" tabindex="0" :aria-label="label"><code v-if="highlighted && text" class="hljs" v-html="highlighted.html" /><code v-else>{{ text || emptyText }}</code></pre>
    </div>
  </div>
</template>

<style scoped>
.text-preview { padding: 16px; }
.text-preview-panel {
  height: min(560px, calc(100dvh - 240px));
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  border: 1px solid var(--sui-border-strong);
  border-radius: 6px;
  overflow: hidden;
  background: var(--sui-bg-hover);
}
.text-preview-toolbar { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: 8px; padding: 8px 12px; border-bottom: 1px solid var(--sui-border-strong); color: var(--color-secondary-text); font-size: 12px; }
.text-preview-toolbar label { display: flex; align-items: center; gap: 8px; }
.text-preview-toolbar select { min-height: 32px; max-width: 180px; padding: 4px 8px; border: 1px solid var(--sui-border-strong); border-radius: 4px; color: var(--color-foreground); background: var(--color-surface); }
.text-preview-content {
  flex: 1;
  min-height: 0;
  margin: 0;
  padding: 12px;
  overflow: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  tab-size: 4;
  color: var(--color-foreground);
  background: transparent;
  font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
  font-size: 13px;
  line-height: 1.6;
  user-select: text;
  -webkit-user-select: text;
}
.text-preview-content :deep(code) { display: block; padding: 0; background: transparent; font: inherit; white-space: inherit; overflow-wrap: inherit; }
@media (max-width: 640px) {
  .text-preview { padding: 12px; }
  .text-preview-toolbar select { min-height: 44px; }
}
</style>
