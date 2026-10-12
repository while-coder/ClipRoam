<script setup lang="ts">
import { ref } from "vue";
import { Check, ChevronDown } from "lucide-vue-next";
import { isMobile } from "../../composables/usePlatform";
import { usePopoverMenu } from "./usePopoverMenu";
import type { EntryFilter } from "../../types";

const model = defineModel<EntryFilter>({ required: true });
const labels: Record<EntryFilter, string> = { all: "全部", text: "文本", files: "文件", image: "图片" };
const options = Object.values(labels);
const typeOptions = Object.entries(labels) as [EntryFilter, string][];
const trigger = ref<HTMLButtonElement>();
const menu = ref<HTMLElement>();
const { menuOpen, menuStyle, toggleMenu, openMenuFromKeyboard, handleMenuKeydown } = usePopoverMenu({
  trigger,
  menu,
  width: 152,
  focusSelector: "[aria-selected='true']",
});

function selectType(value: EntryFilter): void {
  model.value = value;
  menuOpen.value = false;
  trigger.value?.focus();
}

function update(value: string[]): void {
  const match = (Object.entries(labels) as [EntryFilter, string][]).find(([, label]) => label === value[0]);
  model.value = match?.[0] ?? "all";
}
</script>

<template>
  <button
    v-if="isMobile"
    ref="trigger"
    class="entry-type-trigger"
    type="button"
    aria-haspopup="listbox"
    :aria-expanded="menuOpen"
    :aria-label="`类型筛选：${labels[model]}`"
    @click="toggleMenu"
    @keydown.stop
    @keydown.arrow-down.prevent="openMenuFromKeyboard"
    @keydown.arrow-up.prevent="openMenuFromKeyboard"
  >
    <span>{{ model === 'all' ? '全部类型' : labels[model] }}</span>
    <ChevronDown :size="14" aria-hidden="true" />
  </button>
  <STagFilter v-else class="entry-type-filter" :value="[labels[model]]" :options="options" :multiple="false" @update:value="update" />
  <Teleport to="body">
    <div
      v-if="isMobile && menuOpen"
      ref="menu"
      class="entry-type-menu"
      role="listbox"
      aria-label="类型筛选"
      :style="menuStyle"
      @keydown="handleMenuKeydown"
    >
      <button
        v-for="[value, label] in typeOptions"
        :key="value"
        type="button"
        role="option"
        :aria-selected="model === value"
        :class="{ active: model === value }"
        @click="selectType(value)"
      >
        <Check :size="14" :class="{ hidden: model !== value }" aria-hidden="true" />
        <span>{{ value === 'all' ? '全部类型' : label }}</span>
      </button>
    </div>
  </Teleport>
</template>

<style scoped>
.entry-type-filter :deep(.s-tag-filter-clear) { display: none; }
.entry-type-trigger { display: flex; align-items: center; justify-content: space-between; min-width: 96px; height: 44px; gap: 8px; padding: 0 9px 0 12px; color: #cbd5e1; background: rgba(255, 255, 255, 0.04); border: 1px solid rgba(148, 163, 184, 0.18); border-radius: 6px; font-size: 13px; cursor: pointer; white-space: nowrap; }
.entry-type-trigger:hover { color: #e2e8f0; background: rgba(255, 255, 255, 0.07); }
.entry-type-trigger:focus-visible { border-color: #60a5fa; outline: 0; box-shadow: 0 0 0 2px rgba(59, 130, 246, 0.18); }
.entry-type-menu { position: fixed; z-index: 60; display: grid; gap: 2px; padding: 5px; background: #111c31; border: 1px solid rgba(148, 163, 184, 0.24); border-radius: 8px; box-shadow: 0 14px 36px rgba(2, 6, 23, 0.62); }
.entry-type-menu button { display: grid; grid-template-columns: 18px 1fr; align-items: center; min-height: 36px; padding: 0 8px; color: #cbd5e1; text-align: left; background: transparent; border: 0; border-radius: 5px; font-size: 13px; cursor: pointer; }
.entry-type-menu button:hover, .entry-type-menu button:focus-visible { color: #f8fafc; background: rgba(96, 165, 250, 0.13); outline: 0; }
.entry-type-menu button.active { color: #dbeafe; background: #1e3a5f; }
.hidden { visibility: hidden; }
</style>
