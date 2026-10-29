<script setup lang="ts">
import { computed } from "vue";
import type { FavoriteFolder } from "@cliproam/protocol";
import { flattenFavoriteFolders } from "./folderTree";
const props = withDefaults(defineProps<{
  modelValue: string | null;
  folders: FavoriteFolder[];
  nullLabel?: string;
  allowedIds?: string[];
  disabled?: boolean;
}>(), { nullLabel: "未分类" });
const emit = defineEmits<{ "update:modelValue": [value: string | null] }>();
const options = computed(() => flattenFavoriteFolders(props.folders).filter((folder) => !props.allowedIds || props.allowedIds.includes(folder.id)));
</script>

<template>
  <select class="favorite-folder-select" :value="modelValue ?? ''" :disabled="disabled"
    @change="emit('update:modelValue', ($event.target as HTMLSelectElement).value || null)">
    <option value="">{{ nullLabel }}</option>
    <option v-for="folder in options" :key="folder.id" :value="folder.id">{{ folder.path }}</option>
  </select>
</template>

<style scoped>
.favorite-folder-select { width: 100%; min-width: 0; min-height: 36px; padding: 7px 10px; border: 1px solid var(--color-border); border-radius: 6px; background: var(--color-background); color: var(--color-foreground); font: inherit; color-scheme: dark; }
.favorite-folder-select:focus-visible { outline: 2px solid var(--sui-primary); outline-offset: 2px; }
.favorite-folder-select:disabled { opacity: .55; }
:global(.mobile-app) .favorite-folder-select { min-height: 44px; }
</style>
