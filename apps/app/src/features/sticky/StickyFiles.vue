<script setup lang="ts">
import { ref } from "vue";
import { File, Folder, ChevronRight, ChevronDown } from "lucide-vue-next";
import { isFileNode, type FileInfo } from "@cliproam/protocol";
import { formatFileSize } from "@cliproam/ui/bytes";

defineProps<{ tree: FileInfo }>();
const expanded = ref(new Set<string>());
function toggle(name: string): void {
  if (expanded.value.has(name)) expanded.value.delete(name);
  else expanded.value.add(name);
}
</script>

<template>
  <ul class="sticky-files">
    <li v-for="[name, node] in Object.entries(tree)" :key="name">
      <div v-if="isFileNode(node)" class="sticky-file" :title="name">
        <File :size="16" aria-hidden="true" /><span>{{ name }}</span><small>{{ formatFileSize(node.s) }}</small>
      </div>
      <template v-else>
        <button class="sticky-file" :aria-expanded="expanded.has(name)" @click="toggle(name)">
          <component :is="expanded.has(name) ? ChevronDown : ChevronRight" :size="14" aria-hidden="true" />
          <Folder :size="16" aria-hidden="true" /><span>{{ name }}</span><small>{{ Object.keys(node).length }} 项</small>
        </button>
        <StickyFiles v-if="expanded.has(name)" :tree="node" />
      </template>
    </li>
  </ul>
</template>

<style scoped>
.sticky-files { list-style: none; margin: 0; padding: 0; }
.sticky-files .sticky-files { padding-left: 14px; border-left: 1px solid var(--border); margin-left: 7px; }
.sticky-file { display: flex; gap: 7px; align-items: center; width: 100%; padding: 8px 0; color: inherit; text-align: left; border: 0; background: transparent; }
button.sticky-file { cursor: pointer; }
.sticky-file svg { flex-shrink: 0; color: var(--color-secondary-text); }
.sticky-file span { min-width: 0; overflow-wrap: anywhere; }
.sticky-file small { margin-left: auto; white-space: nowrap; color: var(--color-secondary-text); font-size: 12px; }
button.sticky-file:hover { background: var(--sui-bg-hover); }
</style>
