<script setup lang="ts">
import { computed } from "vue";
import type { Device } from "../../types";

/** 空选择表示不过滤（全部设备）。菜单交互统一使用 SSelect 多选。 */
const props = defineProps<{
  modelValue: string[];
  devices: Device[];
}>();

const emit = defineEmits<{
  "update:modelValue": [value: string[]];
}>();

const options = computed(() => props.devices.map((device) => ({ value: device.id, label: device.name })));

const currentLabel = computed(() => {
  if (!props.modelValue.length) return "全部设备";
  const firstName = options.value.find((option) => option.value === props.modelValue[0])?.label;
  if (props.modelValue.length === 1) return firstName ?? "全部设备";
  return `${firstName} 等 ${props.modelValue.length} 台`;
});
</script>

<template>
  <div class="device-filter-control">
    <span class="device-filter-label">设备</span>
    <SSelect
      class="device-filter-trigger"
      :value="modelValue"
      :options="options"
      multiple
      single-line
      :max-tag-count="1"
      :menu-width="200"
      placeholder="全部设备"
      aria-label="设备筛选"
      :title="`设备筛选：${currentLabel}`"
      @update:value="emit('update:modelValue', $event)"
    />
  </div>
</template>

<style scoped>
.device-filter-control { display: flex; flex: 0 0 auto; align-items: center; height: 27px; gap: 5px; margin-left: 5px; padding-left: 9px; border-left: 1px solid rgba(148, 163, 184, 0.16); white-space: nowrap; }
.device-filter-label { color: #64748b; font-size: 11px; }
.device-filter-control .device-filter-trigger { min-width: 106px; width: auto; }
.device-filter-trigger :deep(.s-select) { min-height: 27px; height: 27px; gap: 8px; padding: 0 7px 0 9px; color: #cbd5e1; background: rgba(255, 255, 255, 0.04); border: 1px solid rgba(148, 163, 184, 0.18); border-radius: 6px; font-size: 11px; }
.device-filter-trigger :deep(.s-select:hover) { color: #e2e8f0; background: rgba(255, 255, 255, 0.07); }
.device-filter-trigger :deep(.s-select:focus-visible) { border-color: #60a5fa; outline: 0; box-shadow: 0 0 0 2px rgba(59, 130, 246, 0.18); }
.device-filter-trigger :deep(.s-multi-select-chip) { max-width: 140px; padding: 0; color: inherit; background: transparent; border-radius: 0; font-size: inherit; }
@media (max-width: 640px) and (pointer: coarse) {
  .device-filter-control { height: 44px; margin-left: 0; padding-left: 12px; }
  .device-filter-label { font-size: 13px; }
  .device-filter-control .device-filter-trigger { min-width: 126px; }
  .device-filter-trigger :deep(.s-select) { min-height: 44px; height: 44px; padding-inline: 12px 9px; font-size: 13px; }
}
</style>
