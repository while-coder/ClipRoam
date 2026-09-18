<script setup lang="ts">
import { computed, ref } from "vue";
import { Check, ChevronDown } from "lucide-vue-next";
import { usePopoverMenu } from "./usePopoverMenu";
import type { Device } from "../../types";

/**
 * 历史列表的来源设备多选下拉。骨架与 TimeFilterControl 相同（trigger +
 * Teleport 菜单 + 外点关闭 + 键盘导航），区别在于选中即勾选、菜单不收起，
 * 空选择表示不过滤（全部设备）。
 */
const props = defineProps<{
  modelValue: string[];
  devices: Device[];
}>();

const emit = defineEmits<{
  "update:modelValue": [value: string[]];
}>();

const trigger = ref<HTMLButtonElement>();
const menu = ref<HTMLElement>();
const { menuOpen, menuStyle, toggleMenu, openMenuFromKeyboard, handleMenuKeydown } = usePopoverMenu({
  trigger,
  menu,
  width: 200,
  focusSelector: "button",
});

const options = computed(() => props.devices.map((device) => ({ value: device.id, label: device.name })));
const selected = computed(() => new Set(props.modelValue));

const currentLabel = computed(() => {
  if (!props.modelValue.length) return "全部设备";
  const firstName = options.value.find((option) => option.value === props.modelValue[0])?.label;
  if (props.modelValue.length === 1) return firstName ?? "全部设备";
  return `${firstName} 等 ${props.modelValue.length} 台`;
});

function isSelected(value: string): boolean {
  return selected.value.has(value);
}

function toggleOption(value: string): void {
  const next = isSelected(value)
    ? props.modelValue.filter((id) => id !== value)
    : [...props.modelValue, value];
  emit("update:modelValue", next);
}
</script>

<template>
  <div class="device-filter-control">
    <span class="device-filter-label">设备</span>
    <button
      ref="trigger"
      class="device-filter-trigger"
      type="button"
      aria-haspopup="listbox"
      :aria-expanded="menuOpen"
      :title="`设备筛选：${currentLabel}`"
      @click="toggleMenu"
      @keydown.stop
      @keydown.arrow-down.prevent="openMenuFromKeyboard"
      @keydown.arrow-up.prevent="openMenuFromKeyboard"
    >
      <span>{{ currentLabel }}</span>
      <ChevronDown :size="14" aria-hidden="true" />
    </button>
  </div>

  <Teleport to="body">
    <div
      v-if="menuOpen"
      ref="menu"
      class="device-filter-menu"
      role="listbox"
      aria-label="设备筛选"
      aria-multiselectable="true"
      :style="menuStyle"
      @keydown="handleMenuKeydown"
    >
      <button
        v-for="option in options"
        :key="option.value"
        type="button"
        role="option"
        :aria-selected="isSelected(option.value)"
        :class="{ active: isSelected(option.value) }"
        @click="toggleOption(option.value)"
      >
        <Check :size="14" :class="{ hidden: !isSelected(option.value) }" aria-hidden="true" />
        <span>{{ option.label }}</span>
      </button>
    </div>
  </Teleport>
</template>

<style scoped>
.device-filter-control { display: flex; flex: 0 0 auto; align-items: center; height: 27px; gap: 5px; margin-left: 5px; padding-left: 9px; border-left: 1px solid rgba(148, 163, 184, 0.16); white-space: nowrap; }
.device-filter-label { color: #64748b; font-size: 11px; }
.device-filter-trigger { display: flex; align-items: center; justify-content: space-between; min-width: 106px; height: 27px; gap: 8px; padding: 0 7px 0 9px; color: #cbd5e1; background: rgba(255, 255, 255, 0.04); border: 1px solid rgba(148, 163, 184, 0.18); border-radius: 6px; font-size: 11px; cursor: pointer; }
.device-filter-trigger:hover { color: #e2e8f0; background: rgba(255, 255, 255, 0.07); }
.device-filter-trigger:focus-visible { border-color: #60a5fa; outline: 0; box-shadow: 0 0 0 2px rgba(59, 130, 246, 0.18); }
.device-filter-menu { position: fixed; z-index: 60; display: grid; gap: 2px; max-height: 320px; overflow-y: auto; padding: 5px; background: #111c31; border: 1px solid rgba(148, 163, 184, 0.24); border-radius: 8px; box-shadow: 0 14px 36px rgba(2, 6, 23, 0.62); }
.device-filter-menu button { display: grid; grid-template-columns: 18px 1fr; align-items: center; min-height: 34px; padding: 0 9px; color: #cbd5e1; text-align: left; background: transparent; border: 0; border-radius: 5px; font-size: 12px; cursor: pointer; }
.device-filter-menu button:hover, .device-filter-menu button:focus-visible { color: #f8fafc; background: rgba(96, 165, 250, 0.13); outline: 0; }
.device-filter-menu button.active { color: #dbeafe; background: #1e3a5f; }
.hidden { visibility: hidden; }
@media (max-width: 640px) {
  .device-filter-control { height: 44px; margin-left: 0; padding-left: 12px; }
  .device-filter-label { font-size: 13px; }
  .device-filter-trigger { min-width: 126px; height: 44px; padding-inline: 12px 9px; font-size: 13px; }
  .device-filter-menu button { min-height: 44px; font-size: 14px; }
}
</style>
