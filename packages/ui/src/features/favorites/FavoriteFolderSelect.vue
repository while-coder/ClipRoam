<script setup lang="ts">
import { computed } from "vue";
import SSelect, { type SelectOption } from "@qingfeng346/ui-kit/components/SSelect.vue";
import type { FavoriteFolder } from "@cliproam/protocol";
const props = withDefaults(defineProps<{
  modelValue: string | null;
  folders: FavoriteFolder[];
  nullLabel?: string;
  allowedIds?: string[];
  disabled?: boolean;
}>(), { nullLabel: "未分类" });
const emit = defineEmits<{ "update:modelValue": [value: string | null] }>();
const options = computed<SelectOption[]>(() => {
  function childrenOf(parentId: string | null): SelectOption[] {
    return props.folders.filter((folder) => folder.parentId === parentId).flatMap((folder): SelectOption[] => {
      const children = childrenOf(folder.id);
      const selectable = !props.allowedIds || props.allowedIds.includes(folder.id);
      const self = { label: folder.name, value: folder.id };
      if (!children.length) return selectable ? [self] : [];
      return [{
        label: folder.name,
        children: selectable ? [self, { divider: true }, ...children] : children,
      }];
    });
  }
  return [{ label: props.nullLabel, value: "" }, ...childrenOf(null)];
});
</script>

<template>
  <SSelect class="favorite-folder-select" :value="modelValue ?? ''" :options="options" :placeholder="nullLabel" :disabled="disabled"
    @update:value="(value: string) => emit('update:modelValue', value || null)" />
</template>

<style scoped>
.favorite-folder-select { width: 100%; min-width: 0; }
.favorite-folder-select :deep(.s-select) { min-height: 36px; }
@media (max-width: 768px) { .favorite-folder-select :deep(.s-select) { min-height: 44px; } }
</style>
