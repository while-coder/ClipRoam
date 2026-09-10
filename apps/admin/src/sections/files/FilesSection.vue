<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { deleteFile, fetchFiles, type AdminFile, type FileStats } from "../../shared/api.js";

const pageLimit = 500;

const files = ref<AdminFile[]>([]);
const total = ref(0);
const stats = ref<FileStats>();
const loading = ref(true);
const submitting = ref(false);
const error = ref("");
const notice = ref("");
const search = ref("");
const confirmingFile = ref<AdminFile>();

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
    error.value = reason instanceof Error ? reason.message : "加载文件列表失败。";
  } finally {
    loading.value = false;
  }
}

function requestRemoval(file: AdminFile): void {
  error.value = "";
  notice.value = "";
  confirmingFile.value = file;
}

async function removeFile(): Promise<void> {
  const file = confirmingFile.value;
  if (!file || submitting.value) return;
  submitting.value = true;
  error.value = "";
  notice.value = "";
  try {
    await deleteFile(file.fileId);
    confirmingFile.value = undefined;
    notice.value = "文件已删除。";
    await load();
  } catch (reason) {
    error.value = reason instanceof Error ? reason.message : "删除文件失败。";
  } finally {
    submitting.value = false;
  }
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[unit]}`;
}

function formatDateTime(iso: string): string {
  return new Date(iso).toLocaleString();
}

onMounted(load);
onUnmounted(() => clearTimeout(searchTimer));
</script>

<template>
  <section class="content-section" aria-labelledby="files-title">
    <header class="topbar">
      <h1 id="files-title">文件管理</h1>
      <button class="secondary" type="button" :disabled="loading" @click="load">刷新</button>
    </header>

    <div class="stats-row">
      <section class="status-card">
        <div class="status-copy">
          <p class="label">内容池文件</p>
          <strong>{{ stats ? `${stats.count} 项` : "—" }}</strong>
        </div>
      </section>
      <section class="status-card">
        <div class="status-copy">
          <p class="label">已落盘 / 待上传</p>
          <strong>{{ stats ? `${stats.storedCount} / ${stats.count - stats.storedCount}` : "—" }}</strong>
        </div>
      </section>
      <section class="status-card">
        <div class="status-copy">
          <p class="label">占用空间</p>
          <strong>{{ stats ? formatBytes(stats.storedBytes) : "—" }}</strong>
        </div>
      </section>
    </div>

    <section class="panel">
      <div class="panel-heading">
        <div>
          <h2>内容池</h2>
          <p>文件按内容哈希（sha256）寻址，与剪贴板条目独立存储；删除后引用它的条目将无法再读取该内容。</p>
        </div>
        <input
          v-model="search"
          class="user-search"
          type="search"
          placeholder="按内容 ID 前缀搜索"
          aria-label="按内容 ID 前缀搜索"
          @input="onSearchInput"
        />
      </div>

      <p class="muted result-summary">{{ resultSummary }}</p>
      <p v-if="!loading && files.length === 0 && search.trim()" class="muted">没有匹配的内容 ID。</p>
      <p v-else-if="!loading && files.length === 0" class="muted">内容池还是空的。</p>
      <table v-else-if="!loading" class="user-table file-table">
        <thead>
          <tr>
            <th scope="col">内容 ID</th>
            <th scope="col">大小</th>
            <th scope="col">状态</th>
            <th scope="col">注册时间</th>
            <th scope="col"><span class="visually-hidden">操作</span></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="file in files" :key="file.fileId">
            <td class="mono" :title="file.fileId">{{ file.fileId }}</td>
            <td>{{ formatBytes(file.size) }}</td>
            <td>
              <span class="status-pill" :class="{ active: file.stored }">{{ file.stored ? "已存储" : "待上传" }}</span>
            </td>
            <td>{{ formatDateTime(file.createdAt) }}</td>
            <td class="row-actions">
              <button class="danger" type="button" :disabled="submitting" @click="requestRemoval(file)">删除</button>
            </td>
          </tr>
        </tbody>
      </table>

      <p v-if="error" class="message error" role="alert">{{ error }}</p>
      <p v-if="notice" class="message success" role="status">{{ notice }}</p>
    </section>

    <div v-if="confirmingFile" class="modal-backdrop" role="presentation">
      <section class="confirm-dialog" role="dialog" aria-modal="true" aria-labelledby="delete-file-title">
        <h2 id="delete-file-title">删除文件？</h2>
        <p class="mono break-all">{{ confirmingFile.fileId }}</p>
        <p>将删除磁盘上的这份文件并移除注册记录{{ confirmingFile.stored ? "" : "（字节尚未落盘，仅移除注册记录）" }}。引用它的剪贴板条目会立即失去该内容。</p>
        <p v-if="error" class="message error" role="alert">{{ error }}</p>
        <div class="form-actions">
          <button class="secondary" type="button" :disabled="submitting" @click="confirmingFile = undefined">取消</button>
          <button class="danger" type="button" :disabled="submitting" @click="removeFile">{{ submitting ? "正在删除…" : "确认删除" }}</button>
        </div>
      </section>
    </div>
  </section>
</template>
