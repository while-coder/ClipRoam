<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import {
  Check,
  Clipboard,
  Download,
  File,
  FileText,
  FolderOpen,
  Image,
  LoaderCircle,
  Monitor,
  Search,
  Settings2,
  Trash2,
  Upload,
  X,
} from "lucide-vue-next";
import TimeFilterControl from "../components/TimeFilterControl.vue";
import PaginationControl from "../components/PaginationControl.vue";
import { useHistoryPagination } from "../composables/useHistoryPagination";
import { isPasteWindow, runningInTauri, usePlatform } from "../composables/usePlatform";
import {
  formatAge as formatAgeRelative,
  formatExactDateTime,
  parseLocalDate,
} from "../utils/format";
import {
  canManualUpload,
  canSaveEntry,
  deviceName as deviceDisplayName,
  fileEntrySummary,
  saveEntryLabel,
  syncStatusLabel,
  uploadStatus as uploadStatusOf,
} from "../utils/entry";
import type {
  ClipboardEntry,
  Device,
  DownloadProgress,
  EntryFilter,
  LocalClipboardEntry,
  TimeFilter,
  ToastTone,
  UploadProgress,
} from "../types";

const props = defineProps<{
  entries: LocalClipboardEntry[];
  devicesById: Record<string, Device>;
  syncedEntryIds: Set<string>;
  connectionStatus: { label: string; title: string; tone: string };
  currentTime: number;
  importingShare: boolean;
  activatingEntryId: string;
  uploadingEntryId: string;
  savingEntryId: string;
  uploadProgressByEntryId: Record<string, UploadProgress>;
  downloadProgressByEntryId: Record<string, DownloadProgress>;
  showToast: (message: string, tone?: ToastTone) => void;
  ensureLocalFiles: (entry: LocalClipboardEntry) => Promise<LocalClipboardEntry>;
  clearHistory: () => Promise<void>;
}>();

const emit = defineEmits<{
  activate: [entry: LocalClipboardEntry, viaClick: boolean];
  remove: [entry: ClipboardEntry];
  save: [entry: LocalClipboardEntry];
  upload: [entry: LocalClipboardEntry];
  refresh: [];
  "open-settings": [];
}>();

const { isMobile } = usePlatform();

const query = ref("");
const filter = ref<EntryFilter>("all");
const timeFilter = ref<TimeFilter>("all");
const startDate = ref("");
const endDate = ref("");
const selectedEntryId = ref("");
const clearHistoryConfirmVisible = ref(false);
const clearingHistory = ref(false);
const capturingClipboard = ref(false);
const previewImage = ref<LocalClipboardEntry>();
const previewLoading = ref(false);
const previewDialog = ref<HTMLElement>();
const searchInput = ref<HTMLInputElement>();
const historyListElement = ref<HTMLElement>();
const clearHistoryButton = ref<HTMLButtonElement>();
const clearHistoryCancelButton = ref<HTMLButtonElement>();
const clearHistoryConfirmButton = ref<HTMLButtonElement>();

const timeRangeError = computed(() => {
  if (timeFilter.value !== "custom") return "";
  if (!startDate.value || !endDate.value) return "请选择完整的开始和结束日期";
  if (startDate.value > endDate.value) return "开始日期不能晚于结束日期";
  return "";
});

const activeTimeRange = computed<{ start?: number; end?: number }>(() => {
  if (timeFilter.value === "all") return {};
  if (timeFilter.value === "custom") {
    if (timeRangeError.value) return {};
    return {
      start: parseLocalDate(startDate.value)?.getTime(),
      end: parseLocalDate(endDate.value, true)?.getTime(),
    };
  }
  const start = new Date(props.currentTime);
  start.setHours(0, 0, 0, 0);
  if (timeFilter.value === "7-days") start.setDate(start.getDate() - 6);
  if (timeFilter.value === "30-days") start.setDate(start.getDate() - 29);
  const end = new Date(props.currentTime);
  end.setHours(23, 59, 59, 999);
  return { start: start.getTime(), end: end.getTime() };
});

const timeFilterSummary = computed(() => {
  if (timeFilter.value === "all") return "";
  if (timeFilter.value === "today") return "今天";
  if (timeFilter.value === "7-days") return "近 7 天";
  if (timeFilter.value === "30-days") return "近 30 天";
  if (!startDate.value || !endDate.value) return "自定义区间";
  return `${startDate.value.replace(/-/g, "/")}–${endDate.value.replace(/-/g, "/")}`;
});

const filteredEntries = computed(() => {
  if (timeRangeError.value) return [];
  const needle = query.value.trim().toLocaleLowerCase();
  const timeRange = activeTimeRange.value;
  return props.entries.filter((entry) => {
    const matchesType = filter.value === "all" || entry.kind === filter.value;
    const matchesQuery = !needle
      || entry.content.toLocaleLowerCase().includes(needle)
      || deviceDisplayName(props.devicesById, entry).toLocaleLowerCase().includes(needle);
    const createdAt = new Date(entry.createdAt).getTime();
    const matchesTime = (timeRange.start === undefined || createdAt >= timeRange.start)
      && (timeRange.end === undefined || createdAt <= timeRange.end);
    return matchesType && matchesQuery && matchesTime;
  });
});

const filterResultSummary = computed(() => {
  if (timeRangeError.value) return "日期有误";
  const count = `${filteredEntries.value.length} 条`;
  return timeFilterSummary.value ? `${timeFilterSummary.value} · ${count}` : count;
});

const clearableEntryCount = computed(() => props.entries.length);

watch(filteredEntries, (nextEntries) => {
  if (!nextEntries.some((entry) => entry.id === selectedEntryId.value)) {
    selectedEntryId.value = nextEntries[0]?.id ?? "";
  }
});

const selectedIndex = computed(() => {
  const index = filteredEntries.value.findIndex((entry) => entry.id === selectedEntryId.value);
  return index >= 0 ? index : 0;
});

const {
  page: currentPage,
  pageCount,
  pagedEntries,
  goToPageOf,
  changePage,
} = useHistoryPagination(
  filteredEntries,
  [query, filter, timeFilter, startDate, endDate],
  {
    listElement: historyListElement,
    getSelectedEntryId: () => selectedEntryId.value,
    setSelectedEntryId: (id) => { selectedEntryId.value = id; },
  },
);

function formatAge(createdAt: string): string {
  return formatAgeRelative(createdAt, props.currentTime);
}

function entryDeviceName(entry: ClipboardEntry): string {
  return deviceDisplayName(props.devicesById, entry);
}

function isEntrySynced(entry: ClipboardEntry): boolean {
  return props.syncedEntryIds.has(entry.id);
}

function entryUploadStatus(entry: LocalClipboardEntry): string | undefined {
  return uploadStatusOf(entry, props.uploadProgressByEntryId, props.downloadProgressByEntryId);
}

function entrySaveLabel(entry: LocalClipboardEntry): string {
  return saveEntryLabel(entry, props.savingEntryId, isMobile.value);
}

function imageSource(entry: LocalClipboardEntry): string | undefined {
  const path = entry.summary.previewPath;
  return path && runningInTauri ? convertFileSrc(path) : undefined;
}

function thumbnailSource(entry: LocalClipboardEntry): string | undefined {
  return entry.imageInfo?.thumbnail
    ? `data:image/webp;base64,${entry.imageInfo.thumbnail}`
    : undefined;
}

async function startWindowDrag(event: MouseEvent): Promise<void> {
  if (!runningInTauri || isMobile.value || event.button !== 0) return;
  const target = event.target as HTMLElement;
  if (target.closest("button, input, [role='button']")) return;
  await invoke("start_window_drag");
}

async function focusSearch(): Promise<void> {
  query.value = "";
  if (isPasteWindow) {
    filter.value = "all";
    timeFilter.value = "all";
  }
  selectedEntryId.value = filteredEntries.value[0]?.id ?? "";
  await nextTick();
  searchInput.value?.focus();
}

async function captureCurrentClipboard(): Promise<void> {
  if (!runningInTauri || capturingClipboard.value) return;
  capturingClipboard.value = true;
  try {
    const captured = await invoke<boolean>("capture_current_clipboard_text");
    emit("refresh");
    props.showToast(captured ? "已读取当前文本剪贴板" : "当前剪贴板没有可读取的文本", captured ? "success" : "info");
  } catch (error) {
    props.showToast(`读取剪贴板失败：${error instanceof Error ? error.message : String(error)}`, "error");
  } finally {
    capturingClipboard.value = false;
  }
}

async function openImagePreview(entry: LocalClipboardEntry): Promise<void> {
  if (isPasteWindow || previewLoading.value) return;
  previewLoading.value = true;
  try {
    const localEntry = await props.ensureLocalFiles(entry);
    if (!imageSource(localEntry)) throw new Error("图片文件不可用");
    previewImage.value = localEntry;
    await nextTick();
    previewDialog.value?.focus();
  } catch (error) {
    props.showToast(`无法预览图片：${error instanceof Error ? error.message : String(error)}`, "error");
  } finally {
    previewLoading.value = false;
  }
}

function closeImagePreview(): void {
  previewImage.value = undefined;
  void nextTick(() => searchInput.value?.focus());
}

function selectOrActivate(entry: LocalClipboardEntry): void {
  selectedEntryId.value = entry.id;
  if (isPasteWindow || isMobile.value) emit("activate", entry, true);
}

function activateSelectedEntry(entry?: LocalClipboardEntry): void {
  if (!entry) return;
  emit("activate", entry, false);
}

function moveSelection(offset: -1 | 1): void {
  if (!filteredEntries.value.length) return;
  const index = Math.min(
    Math.max(selectedIndex.value + offset, 0),
    filteredEntries.value.length - 1,
  );
  selectedEntryId.value = filteredEntries.value[index].id;
  goToPageOf(index);
}

async function requestClearHistory(): Promise<void> {
  if (!clearableEntryCount.value) return;
  clearHistoryConfirmVisible.value = true;
  await nextTick();
  clearHistoryCancelButton.value?.focus();
}

function resetTimeFilter(): void {
  timeFilter.value = "all";
  startDate.value = "";
  endDate.value = "";
}

async function closeClearHistoryConfirm(): Promise<void> {
  if (clearingHistory.value) return;
  clearHistoryConfirmVisible.value = false;
  await nextTick();
  clearHistoryButton.value?.focus();
}

async function confirmClearHistory(): Promise<void> {
  if (clearingHistory.value || !clearableEntryCount.value) return;
  const clearedCount = clearableEntryCount.value;
  clearingHistory.value = true;
  try {
    await props.clearHistory();
    clearHistoryConfirmVisible.value = false;
    props.showToast(`已清除 ${clearedCount} 条未固定记录`, "success");
    await nextTick();
    searchInput.value?.focus();
  } catch (error) {
    props.showToast(`清除历史失败：${error instanceof Error ? error.message : String(error)}`, "error");
  } finally {
    clearingHistory.value = false;
  }
}

/**
 * Shared document-keydown hook: the App-level handler delegates here after its
 * own dialogs (settings, setup) had a chance to consume the key. Returns true
 * when the key was handled and should not fall through to window hiding.
 */
function handleKeydown(event: KeyboardEvent): boolean {
  if (clearHistoryConfirmVisible.value) {
    if (event.key === "Escape") {
      event.preventDefault();
      void closeClearHistoryConfirm();
    } else if (event.key === "Tab") {
      event.preventDefault();
      const cancelButton = clearHistoryCancelButton.value;
      const confirmButton = clearHistoryConfirmButton.value;
      const focusConfirm = event.shiftKey
        ? document.activeElement === cancelButton
        : document.activeElement !== confirmButton;
      (focusConfirm ? confirmButton : cancelButton)?.focus();
    }
    return true;
  }
  if (previewImage.value) {
    if (event.key === "Escape") {
      event.preventDefault();
      closeImagePreview();
    }
    return true;
  }
  if (event.key === "ArrowDown") {
    event.preventDefault();
    moveSelection(1);
    return true;
  }
  if (event.key === "ArrowUp") {
    event.preventDefault();
    moveSelection(-1);
    return true;
  }
  if (event.key === "Enter" && !event.shiftKey) {
    event.preventDefault();
    activateSelectedEntry(filteredEntries.value[selectedIndex.value]);
    return true;
  }
  return false;
}

defineExpose({ handleKeydown, focusSearch });
</script>

<template>
  <section class="app-content history-content">
    <div v-if="isPasteWindow" class="paste-drag-strip" aria-hidden="true" @mousedown.left="startWindowDrag"></div>
    <header v-else class="titlebar workspace-titlebar">
      <div class="page-title">
        <span>工作区</span>
        <h1>剪贴板历史</h1>
      </div>
      <div class="titlebar-actions">
        <span v-if="isMobile" class="mobile-connection" :class="connectionStatus.tone">{{ connectionStatus.label }}</span>
        <button v-if="isMobile" class="icon-button" type="button" title="设置" aria-label="打开设置" @click="emit('open-settings')">
          <Settings2 :size="19" />
        </button>
      </div>
    </header>

    <section class="toolbar">
      <div v-if="isMobile && importingShare" class="mobile-share-status" role="status" aria-live="polite" aria-atomic="true">
        <LoaderCircle :size="18" class="spin" aria-hidden="true" />
        <span>正在接收系统分享…</span>
      </div>
      <label class="search-field">
        <Search :size="17" aria-hidden="true" />
        <input ref="searchInput" v-model="query" type="search" placeholder="搜索剪贴板历史" aria-label="搜索剪贴板历史" />
        <kbd>Enter</kbd>
      </label>
      <button v-if="isMobile" class="mobile-capture-button" type="button" :disabled="capturingClipboard" @click="captureCurrentClipboard">
        <LoaderCircle v-if="capturingClipboard" :size="18" class="spin" aria-hidden="true" />
        <Clipboard v-else :size="18" aria-hidden="true" />
        {{ capturingClipboard ? "正在读取…" : "读取当前剪贴板" }}
      </button>
      <div class="filter-row" role="group" aria-label="剪贴板筛选">
        <div class="filter-scroll">
          <button :class="{ active: filter === 'all' }" type="button" @click="filter = 'all'">全部</button>
          <button :class="{ active: filter === 'text' }" type="button" @click="filter = 'text'">文本</button>
          <button :class="{ active: filter === 'files' }" type="button" @click="filter = 'files'">文件</button>
          <button :class="{ active: filter === 'image' }" type="button" @click="filter = 'image'">图片</button>
          <TimeFilterControl
            v-model="timeFilter"
            v-model:start-date="startDate"
            v-model:end-date="endDate"
            :error="timeRangeError"
          />
        </div>
        <div class="filter-actions">
          <span class="result-summary" :class="{ error: timeRangeError }" :title="filterResultSummary">{{ filterResultSummary }}</span>
          <button
            v-if="!isPasteWindow"
            ref="clearHistoryButton"
            class="clear-button"
            type="button"
            :disabled="!clearableEntryCount"
            :title="clearableEntryCount ? `清除 ${clearableEntryCount} 条记录` : '没有可清除的记录'"
            @click="requestClearHistory"
          >清除</button>
        </div>
      </div>
    </section>

    <section id="history-content" ref="historyListElement" class="history-list" aria-label="剪贴板历史">
      <div
        v-for="entry in pagedEntries"
        :key="entry.id"
        class="history-item"
        :class="{ selected: selectedEntryId === entry.id, 'image-entry': entry.kind === 'image' }"
        role="button"
        :tabindex="activatingEntryId === entry.id ? -1 : 0"
        :aria-disabled="activatingEntryId === entry.id"
        @mouseenter="selectedEntryId = entry.id"
        @dblclick="!isPasteWindow && !isMobile && activateSelectedEntry(entry)"
        @click="selectOrActivate(entry)"
        @keydown.enter.stop="activateSelectedEntry(entry)"
        @keydown.space.prevent.stop="activateSelectedEntry(entry)"
      >
        <button
          v-if="entry.kind === 'image' && !isPasteWindow"
          class="image-thumbnail"
          type="button"
          :aria-label="`预览${entry.content}`"
          :title="`预览${entry.content}`"
          @click.stop="openImagePreview(entry)"
          @dblclick.stop
        >
          <img v-if="thumbnailSource(entry)" :src="thumbnailSource(entry)" :alt="entry.content" loading="lazy" />
          <Image v-else :size="18" aria-hidden="true" />
        </button>
        <span v-else-if="entry.kind === 'image' && thumbnailSource(entry)" class="image-thumbnail" aria-hidden="true">
          <img :src="thumbnailSource(entry)" alt="" loading="lazy" />
        </span>
        <span v-else class="kind-icon">
          <LoaderCircle v-if="activatingEntryId === entry.id" :size="18" class="spin" />
          <FileText v-else-if="entry.kind === 'text'" :size="18" />
          <File v-else-if="entry.kind === 'files' && entry.summary.rootKind === 'file'" :size="18" />
          <FolderOpen v-else-if="entry.kind === 'files'" :size="18" />
          <Image v-else-if="entry.kind === 'image'" :size="18" />
          <Clipboard v-else :size="18" />
        </span>
        <span class="entry-body">
          <span class="entry-content">{{ entry.content }}</span>
          <span class="entry-meta">
            <Monitor :size="12" /> {{ entryDeviceName(entry) }}
            <span>·</span>
            <span :title="formatExactDateTime(entry.createdAt)">{{ formatAge(entry.createdAt) }}</span>
            <span>·</span>
            <span class="sync-status" role="img" :title="syncStatusLabel(isEntrySynced(entry))" :aria-label="syncStatusLabel(isEntrySynced(entry))">{{ isEntrySynced(entry) ? "☁️" : "⏳" }}</span>
            <template v-if="fileEntrySummary(entry)">
              <span>·</span>
              <span>{{ fileEntrySummary(entry) }}</span>
            </template>
            <template v-if="entryUploadStatus(entry)">
              <span>·</span>
              <span class="upload-status" :class="{ uploaded: entryUploadStatus(entry) === '已上传', uploading: entryUploadStatus(entry)?.startsWith('上传中') }">{{ entryUploadStatus(entry) }}</span>
            </template>
          </span>
        </span>
        <span v-if="!isPasteWindow" class="entry-actions">
          <span
            v-if="canSaveEntry(entry)"
            class="item-action"
            role="button"
            tabindex="0"
            :title="entrySaveLabel(entry)"
            :aria-label="entrySaveLabel(entry)"
            @click.stop="emit('save', entry)"
            @keydown.enter.stop="emit('save', entry)"
          ><LoaderCircle v-if="savingEntryId === entry.id" :size="15" class="spin" /><Download v-else :size="15" /></span>
          <span
            v-if="canManualUpload(entry)"
            class="item-action"
            role="button"
            tabindex="0"
            :title="uploadingEntryId === entry.id ? '正在上传…' : '上传到服务器（小于 100 MB）'"
            :aria-label="uploadingEntryId === entry.id ? '正在上传' : '上传到服务器'"
            @click.stop="emit('upload', entry)"
            @keydown.enter.stop="emit('upload', entry)"
          ><LoaderCircle v-if="uploadingEntryId === entry.id || uploadProgressByEntryId[entry.id]" :size="15" class="spin" /><Upload v-else :size="15" /></span>
          <span
            class="item-action danger"
            role="button"
            tabindex="0"
            title="删除"
            aria-label="删除"
            @click.stop="emit('remove', entry)"
            @keydown.enter.stop="emit('remove', entry)"
          ><Trash2 :size="15" /></span>
        </span>
      </div>

      <div v-if="!filteredEntries.length" class="empty-state">
        <Search :size="28" />
        <strong>{{ timeRangeError ? "日期区间无效" : timeFilter !== "all" ? "该时间段暂无内容" : "没有匹配内容" }}</strong>
        <span>{{ timeRangeError || (timeFilter !== "all" ? "可以更换时间范围，或清除时间筛选查看全部记录" : isMobile ? "其他设备的内容同步后会显示在这里" : "复制文本后会自动保存到这里") }}</span>
        <button v-if="timeFilter !== 'all'" class="empty-filter-reset" type="button" @click="resetTimeFilter">清除时间筛选</button>
      </div>
    </section>

    <footer class="footer-hint">
      <span v-if="isMobile">点按文本复制，点按文件下载到缓存</span>
      <span v-else-if="isPasteWindow">单击记录立即粘贴</span>
      <span v-else>单击选择，双击复制</span>
      <span v-if="!isMobile"><kbd>↑</kbd><kbd>↓</kbd> 选择</span>
      <span v-if="!isMobile"><kbd>Enter</kbd> {{ isPasteWindow ? "粘贴" : "复制" }}</span>
      <span v-if="!isMobile"><kbd>Esc</kbd> 关闭</span>
      <PaginationControl
        :page="currentPage"
        :page-count="pageCount"
        :total="filteredEntries.length"
        @update:page="changePage"
      />
      <span v-if="!isPasteWindow" class="privacy"><Check :size="13" /> 本地优先</span>
    </footer>

    <div v-if="clearHistoryConfirmVisible" class="confirm-backdrop" @mousedown.self="closeClearHistoryConfirm">
      <section class="confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="clear-history-heading" aria-describedby="clear-history-description">
        <span class="confirm-icon danger" aria-hidden="true"><Trash2 :size="20" /></span>
        <div class="confirm-copy">
          <h2 id="clear-history-heading">清除未固定记录？</h2>
          <p id="clear-history-description">将永久删除 {{ clearableEntryCount }} 条未固定的剪贴板记录。已固定记录会保留，此操作无法撤销。</p>
        </div>
        <footer class="confirm-actions">
          <button ref="clearHistoryCancelButton" class="secondary-button" type="button" :disabled="clearingHistory" @click="closeClearHistoryConfirm">取消</button>
          <button ref="clearHistoryConfirmButton" class="danger-button" type="button" :disabled="clearingHistory || !clearableEntryCount" @click="confirmClearHistory">
            <LoaderCircle v-if="clearingHistory" :size="17" class="spin" aria-hidden="true" />
            <Trash2 v-else :size="17" aria-hidden="true" />
            {{ clearingHistory ? "正在清除…" : "确认清除" }}
          </button>
        </footer>
      </section>
    </div>

    <div v-if="previewImage" class="image-preview-backdrop" @mousedown.self="closeImagePreview">
      <section ref="previewDialog" class="image-preview-dialog" role="dialog" aria-modal="true" :aria-label="previewImage.content" tabindex="-1">
        <header class="image-preview-header">
          <div>
            <span>图片预览</span>
            <strong>{{ previewImage.content }}</strong>
          </div>
          <button class="icon-button" type="button" title="关闭预览" aria-label="关闭图片预览" @click="closeImagePreview">
            <X :size="17" />
          </button>
        </header>
        <div class="image-preview-stage">
          <img :src="imageSource(previewImage)" :alt="previewImage.content" />
        </div>
      </section>
    </div>
  </section>
</template>
