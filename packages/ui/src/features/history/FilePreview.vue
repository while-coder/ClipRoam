<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { fullEntry } from "./useHistorySync";
import { isFileNode, type FileInfo, type TreeNode } from "@cliproam/protocol";
import { ChevronDown, ChevronRight, File, Folder, FolderOpen } from "lucide-vue-next";
import { errorMessage } from "../../utils/error";
import { formatFileSize } from "../../utils/format";
import { fileEntrySummary } from "../../utils/entry";
import type { LocalClipboardEntry } from "../../types";

const props = defineProps<{ entry: LocalClipboardEntry }>();
const emit = defineEmits<{ close: [] }>();
const fileInfo = ref<FileInfo>();
const loading = ref(true);
const error = ref("");
const expanded = ref(new Set<string>());
let disposed = false;

onMounted(async () => {
  try {
    // 列表只有摘要；按需读取完整树，不下载文件内容。
    const entry = await fullEntry(props.entry);
    if (disposed) return;
    if (!entry.fileInfo) throw new Error("文件目录信息不可用");
    fileInfo.value = entry.fileInfo;
  } catch (cause) {
    if (!disposed) error.value = `无法预览文件：${errorMessage(cause)}`;
  } finally {
    if (!disposed) loading.value = false;
  }
});
onBeforeUnmount(() => { disposed = true; });

interface PreviewRow {
  name: string;
  path: string;
  depth: number;
  node: TreeNode;
  directory: boolean;
  detail: string;
}

// 只生成已展开的行；不为折叠目录挂载子树，深层目录用栈遍历。
const rows = computed(() => {
  const result: PreviewRow[] = [];
  const pending: Array<{ name: string; path: string; depth: number; node: TreeNode }> = [];
  const pushChildren = (children: FileInfo, path: string, depth: number): void => {
    const sorted = Object.entries(children).sort(([nameA, nodeA], [nameB, nodeB]) =>
      Number(isFileNode(nodeA)) - Number(isFileNode(nodeB)) || nameA.localeCompare(nameB, "zh-CN", { numeric: true }),
    );
    for (let index = sorted.length - 1; index >= 0; index--) {
      const [name, node] = sorted[index]!;
      pending.push({ name, node, path: path ? `${path}/${name}` : name, depth });
    }
  };
  pushChildren(fileInfo.value ?? {}, "", 0);
  while (pending.length) {
    const row = pending.pop()!;
    const file = isFileNode(row.node);
    const childCount = file ? 0 : Object.keys(row.node).length;
    result.push({
      ...row,
      directory: !file,
      detail: isFileNode(row.node) ? formatFileSize(row.node.s) : childCount ? `${childCount} 项` : "空目录",
    });
    if (!isFileNode(row.node) && expanded.value.has(row.path)) {
      pushChildren(row.node, row.path, row.depth + 1);
    }
  }
  return result;
});

function toggleDirectory(path: string): void {
  if (expanded.value.has(path)) expanded.value.delete(path);
  else expanded.value.add(path);
}
</script>

<template>
  <SModal :show="true" :title="entry.content" width="760px" @update:show="(value: boolean) => { if (!value) emit('close') }">
    <div class="file-preview">
      <p class="file-preview-summary">{{ fileEntrySummary(entry) }}</p>
      <div v-if="loading" class="preview-status" role="status">
        <span class="s-spinner" aria-hidden="true" />正在加载目录…
      </div>
      <div v-else-if="error" class="preview-status" role="alert">{{ error }}</div>
      <ul v-else-if="rows.length" class="file-preview-list" aria-label="文件目录结构">
        <li v-for="row in rows" :key="row.path">
          <button
            v-if="row.directory"
            class="file-preview-row directory"
            type="button"
            :style="{ paddingLeft: `${12 + row.depth * 20}px` }"
            :aria-expanded="expanded.has(row.path)"
            :title="row.path"
            @click="toggleDirectory(row.path)"
          >
            <component :is="expanded.has(row.path) ? ChevronDown : ChevronRight" :size="14" aria-hidden="true" />
            <component :is="expanded.has(row.path) ? FolderOpen : Folder" :size="17" aria-hidden="true" />
            <span class="file-preview-name">{{ row.name }}</span>
            <span class="file-preview-detail">{{ row.detail }}</span>
          </button>
          <div v-else class="file-preview-row" :style="{ paddingLeft: `${12 + row.depth * 20}px` }" :title="row.path">
            <span aria-hidden="true" />
            <File :size="17" aria-hidden="true" />
            <span class="file-preview-name">{{ row.name }}</span>
            <span class="file-preview-detail">{{ row.detail }}</span>
          </div>
        </li>
      </ul>
      <div v-else class="preview-status">没有文件或目录</div>
    </div>
  </SModal>
</template>

<style scoped>
.file-preview { padding: 16px; }
.file-preview-summary { margin: 0 0 12px; color: var(--color-secondary-text); font-size: 13px; }
.preview-status { display: flex; align-items: center; justify-content: center; gap: 8px; min-height: 160px; color: var(--color-secondary-text); }
.file-preview-list { max-height: min(560px, calc(100dvh - 220px)); margin: 0; padding: 0; overflow: auto; list-style: none; }
.file-preview-row { display: grid; grid-template-columns: 14px 17px minmax(100px, 1fr) auto; align-items: center; gap: 8px; width: 100%; min-height: 36px; padding: 6px 12px; border: 0; border-radius: 5px; background: transparent; color: var(--color-foreground); font-size: 13px; text-align: left; }
.directory { cursor: pointer; }
.directory:hover { background: var(--sui-bg-hover); }
.directory:focus-visible { outline-offset: -2px; }
.file-preview-name { overflow-wrap: anywhere; }
.file-preview-detail { color: var(--color-secondary-text); white-space: nowrap; }
@media (max-width: 640px) {
  .file-preview { padding: 12px; }
  .file-preview-row { min-height: 44px; }
}
</style>
