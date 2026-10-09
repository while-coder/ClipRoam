<script setup lang="ts">
import { computed, h, onMounted, onUnmounted, ref } from "vue";
import { confirm, toast, SButton, STag } from "@qingfeng346/ui-kit";
import { deleteFile, fetchFiles, type AdminFile, type FileStats } from "../../shared/api.js";
import { errorMessage } from "../../shared/errorMessage.js";

const files = ref<AdminFile[]>([]);
const total = ref(0);
const stats = ref<FileStats>();
const loading = ref(true);
const submitting = ref(false);
const error = ref("");
const search = ref("");

const resultSummary = computed(() => {
  if (loading.value) return "正在加载文件列表…";
  if (total.value > files.value.length) {
    return `匹配 ${total.value} 项，显示最近注册的前 ${files.value.length} 项，请用内容 ID 前缀缩小范围`;
  }
  return `共 ${total.value} 项`;
});

const searchDebounceMs = 250;
let searchTimer: ReturnType<typeof setTimeout> | undefined;

function onSearchInput(): void {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(load, searchDebounceMs);
}

async function load(): Promise<void> {
  loading.value = true;
  error.value = "";
  try {
    const result = await fetchFiles(search.value);
    files.value = result.files;
    total.value = result.total;
    stats.value = result.stats;
  } catch (reason) {
    error.value = errorMessage(reason, "加载文件列表失败。");
  } finally {
    loading.value = false;
  }
}

async function removeFile(file: AdminFile): Promise<void> {
  if (submitting.value) return;
  const confirmed = await confirm.show({
    title: "删除文件？",
    content: `${file.fileId}\n将删除磁盘上的这份文件并移除注册记录${
      file.stored ? "" : "（字节尚未落盘，仅移除注册记录）"
    }。引用它的剪贴板条目会立即失去该内容。`,
    confirmText: "确认删除",
    error: true,
  });
  if (!confirmed) return;
  submitting.value = true;
  error.value = "";
  try {
    await deleteFile(file.fileId);
    toast.show("success", "文件已删除。");
    await load();
  } catch (reason) {
    error.value = errorMessage(reason, "删除文件失败。");
  } finally {
    submitting.value = false;
  }
}

// 与 apps/app/src/utils/format.ts 的 formatFileSize 是同一实现，改动请同步。
function formatBytes(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

function formatDateTime(iso: string): string {
  return new Date(iso).toLocaleString();
}

// 排序交给 SDataTable 的 column.sorter（组件内部管理方向与 aria-sort），
// 替代原先手写的 sortState/toggleSort/sortArrow/ariaSort。
const columns = [
  {
    title: "内容 ID",
    key: "fileId",
    render: (file: AdminFile) => h("span", { class: "mono file-id-cell", title: file.fileId }, file.fileId),
  },
  { title: "大小", key: "size", sorter: (left: AdminFile, right: AdminFile) => left.size - right.size, render: (file: AdminFile) => formatBytes(file.size) },
  {
    title: "状态",
    key: "stored",
    sorter: (left: AdminFile, right: AdminFile) => Number(left.stored) - Number(right.stored),
    render: (file: AdminFile) => h(STag, { type: file.stored ? "success" : "default" }, () => (file.stored ? "已存储" : "待上传")),
  },
  { title: "注册时间", key: "createdAt", sorter: (left: AdminFile, right: AdminFile) => left.createdAt.localeCompare(right.createdAt), render: (file: AdminFile) => formatDateTime(file.createdAt) },
  {
    title: "操作",
    key: "ops",
    align: "right",
    render: (file: AdminFile) =>
      h(SButton, { type: "error", disabled: submitting.value, onClick: () => removeFile(file) }, () => "删除"),
  },
];

onMounted(load);
onUnmounted(() => clearTimeout(searchTimer));
</script>

<template>
  <section class="content-section" aria-labelledby="files-title">
    <header class="topbar">
      <h1 id="files-title">文件管理</h1>
      <SButton :disabled="loading" @click="load">刷新</SButton>
    </header>

    <SGrid :cols="3" :x-gap="16" :y-gap="16">
      <SCard>
        <div class="status-row">
          <div class="status-copy">
            <p class="label">内容池文件</p>
            <strong>{{ stats ? `${stats.count} 项` : "—" }}</strong>
          </div>
        </div>
      </SCard>
      <SCard>
        <div class="status-row">
          <div class="status-copy">
            <p class="label">已落盘 / 待上传</p>
            <strong>{{ stats ? `${stats.storedCount} / ${stats.count - stats.storedCount}` : "—" }}</strong>
          </div>
        </div>
      </SCard>
      <SCard>
        <div class="status-row">
          <div class="status-copy">
            <p class="label">占用空间</p>
            <strong>{{ stats ? formatBytes(stats.storedBytes) : "—" }}</strong>
          </div>
        </div>
      </SCard>
    </SGrid>

    <SCard title="内容池">
      <template #header-extra>
        <SInput
          v-model:value="search"
          class="user-search"
          type="search"
          placeholder="按内容 ID 前缀搜索"
          aria-label="按内容 ID 前缀搜索"
          clearable
          @update:value="onSearchInput"
        />
      </template>

      <p class="muted panel-desc">文件按内容哈希（sha256）寻址，与剪贴板条目独立存储；删除后引用它的条目将无法再读取该内容。</p>
      <p class="muted result-summary">{{ resultSummary }}</p>
      <SEmpty v-if="!loading && files.length === 0 && search.trim()">没有匹配的内容 ID。</SEmpty>
      <SEmpty v-else-if="!loading && files.length === 0">内容池还是空的。</SEmpty>
      <SDataTable v-else-if="!loading" :columns="columns" :data="files" row-key="fileId" />

      <SAlert v-if="error" type="error">{{ error }}</SAlert>
    </SCard>
  </section>
</template>