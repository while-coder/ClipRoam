<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import {
  Clipboard,
  FilePlus,
  FolderPlus,
  Image,
  LoaderCircle,
  Monitor,
  Search,
  Settings2,
  X,
} from "lucide-vue-next";
import TimeFilterControl from "./TimeFilterControl.vue";
import EntryContextMenu from "./EntryContextMenu.vue";
import EntryKindIcon from "./EntryKindIcon.vue";
import DeviceFilterControl from "./DeviceFilterControl.vue";
import PaginationControl from "./PaginationControl.vue";
import { useHistoryManifest } from "./useHistoryManifest";
import { isPasteWindow, usePlatform } from "../../composables/usePlatform";
import { showToast } from "../toast/useToast";
import { errorMessage } from "../../utils/error";
import {
  formatAge as formatAgeRelative,
  formatExactDateTime,
  parseLocalDate,
  percentOf,
  TIME_FILTER_LABELS,
  validateDateRange,
} from "../../utils/format";
import {
  deviceName as deviceDisplayName,
  fileEntrySummary,
  uploadStatus as uploadStatusOf,
} from "../../utils/entry";
import type {
  ClipboardEntry,
  Device,
  DownloadProgress,
  EntriesManifestFilter,
  EntriesManifestPage,
  EntryFilter,
  LocalClipboardEntry,
  TimeFilter,
  UploadProgress,
} from "../../types";

const props = defineProps<{
  /** Server-style manifest fetch: filtering and paging run in Rust. */
  fetchManifest: (
    filter: EntriesManifestFilter,
    deviceNames: Record<string, string>,
  ) => Promise<EntriesManifestPage>;
  /** Bumped whenever the history may have changed in the background. */
  revision: number;
  devicesById: Record<string, Device>;
  connectionStatus: { label: string; title: string; tone: string };
  currentTime: number;
  importingShare: boolean;
  activatingEntryIds: ReadonlySet<string>;
  savingEntryId: string;
  uploadProgressByEntryId: Record<string, UploadProgress>;
  downloadProgressByEntryId: Record<string, DownloadProgress>;
  ensureLocalFiles: (entry: LocalClipboardEntry) => Promise<LocalClipboardEntry>;
}>();

const emit = defineEmits<{
  activate: [entry: LocalClipboardEntry, viaClick: boolean];
  remove: [entry: ClipboardEntry];
  save: [entry: LocalClipboardEntry];
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
const capturingClipboard = ref(false);
const uploadingFiles = ref(false);
const menuEntry = ref<LocalClipboardEntry>();
const menuX = ref(0);
const menuY = ref(0);
const previewImage = ref<LocalClipboardEntry>();
const previewLoading = ref(false);
const previewDialog = ref<HTMLElement>();
const searchInput = ref<HTMLInputElement>();
const historyListElement = ref<HTMLElement>();

const timeRangeError = computed(() => (
  timeFilter.value === "custom" ? validateDateRange(startDate.value, endDate.value) : ""
));

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
  if (timeFilter.value !== "custom") return TIME_FILTER_LABELS[timeFilter.value];
  if (!startDate.value || !endDate.value) return "自定义区间";
  return `${startDate.value.replace(/-/g, "/")}–${endDate.value.replace(/-/g, "/")}`;
});

// 搜索关键词按 Enter 提交（含输入法 composition 结束）；输入过程不打扰列表。
const committedQuery = ref("");
function commitSearch(): void {
  if (committedQuery.value === query.value.trim()) {
    // 关键词没变（如清空后原样回车）也允许显式重查。
    void fetchManifestPage(1, true);
    return;
  }
  committedQuery.value = query.value.trim();
}
onBeforeUnmount(() => { cancelLongPress(); });

const deviceNames = computed(() =>
  Object.fromEntries(Object.entries(props.devicesById).map(([id, device]) => [id, device.name])),
);
const devices = computed(() => Object.values(props.devicesById));
const selectedDeviceIds = ref<string[]>([]);

const {
  page: currentPage,
  total: manifestTotal,
  pageCount,
  entries: pageEntries,
  fetch: fetchManifestPage,
  clear: clearManifest,
  changePage,
} = useHistoryManifest({
  fetchManifest: props.fetchManifest,
  deviceNames: () => deviceNames.value,
  buildFilter: (page) => ({
    query: committedQuery.value,
    kind: filter.value,
    start: activeTimeRange.value.start,
    end: activeTimeRange.value.end,
    deviceIds: selectedDeviceIds.value,
    page,
  }),
  revision: computed(() => props.revision),
  filterSources: [committedQuery, filter, timeFilter, startDate, endDate, selectedDeviceIds],
  // An invalid custom range matches nothing — the backend never sees it, and
  // background revision bumps must not refill the cleared list either.
  canFetch: () => !timeRangeError.value,
  onError: (message) => showToast(`读取历史失败：${message}`, "error"),
  listElement: historyListElement,
  getSelectedEntryId: () => selectedEntryId.value,
  setSelectedEntryId: (id) => { selectedEntryId.value = id; },
});

// An invalid custom range matches nothing — the backend never sees it.
watch(timeRangeError, (error) => { if (error) clearManifest(); });

const filterResultSummary = computed(() => {
  if (timeRangeError.value) return "日期有误";
  const count = `${manifestTotal.value} 条`;
  return timeFilterSummary.value ? `${timeFilterSummary.value} · ${count}` : count;
});

const selectedLocalIndex = computed(() =>
  pageEntries.value.findIndex((entry) => entry.id === selectedEntryId.value),
);

function formatAge(createdAt: string): string {
  return formatAgeRelative(createdAt, props.currentTime);
}

function entryUploadStatus(entry: LocalClipboardEntry): string | undefined {
  return uploadStatusOf(entry, props.uploadProgressByEntryId, props.downloadProgressByEntryId);
}

/** 下载中的条目在行内显示进度条；排队中 receivedBytes 为 0，从 0% 起步。 */
function entryDownloadProgress(entry: LocalClipboardEntry): DownloadProgress | undefined {
  return props.downloadProgressByEntryId[entry.id];
}

function downloadPercentOf(progress: DownloadProgress): number {
  return percentOf(progress.receivedBytes, progress.totalBytes);
}


function imageSource(entry: LocalClipboardEntry): string | undefined {
  const path = entry.summary.previewPath;
  return path ? convertFileSrc(path) : undefined;
}

function thumbnailSource(entry: LocalClipboardEntry): string | undefined {
  return entry.imageInfo?.thumbnail
    ? `data:image/webp;base64,${entry.imageInfo.thumbnail}`
    : undefined;
}

async function startWindowDrag(event: MouseEvent): Promise<void> {
  if (isMobile.value || event.button !== 0) return;
  const target = event.target as HTMLElement;
  if (target.closest("button, input, select, textarea, kbd, [role='button']")) return;
  await invoke("start_window_drag");
}

async function focusSearch(): Promise<void> {
  query.value = "";
  committedQuery.value = "";
  if (isPasteWindow) {
    filter.value = "all";
    timeFilter.value = "all";
  }
  await fetchManifestPage(1, true);
  selectedEntryId.value = pageEntries.value[0]?.id ?? "";
  await nextTick();
  searchInput.value?.focus();
}

/** 只把焦点还给搜索框；不清搜索词、不重拉列表（关设置弹窗等场景用）。 */
function focusSearchInput(): void {
  searchInput.value?.focus();
}

async function captureCurrentClipboard(): Promise<void> {
  if (capturingClipboard.value) return;
  capturingClipboard.value = true;
  try {
    const captured = await invoke<boolean>("capture_current_clipboard_text");
    emit("refresh");
    showToast(captured ? "已读取当前文本剪贴板" : "当前剪贴板没有可读取的文本", captured ? "success" : "info");
  } catch (error) {
    showToast(`读取剪贴板失败：${errorMessage(error)}`, "error");
  } finally {
    capturingClipboard.value = false;
  }
}

async function captureFilesFromPicker(mode: "file" | "folder"): Promise<void> {
  if (uploadingFiles.value) return;
  uploadingFiles.value = true;
  try {
    const captured = await invoke<boolean>("capture_files_from_picker", { mode });
    emit("refresh");
    showToast(captured ? "已加入上传队列" : "未选择文件", captured ? "success" : "info");
  } catch (error) {
    showToast(`选择文件失败：${errorMessage(error)}`, "error");
  } finally {
    uploadingFiles.value = false;
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
    showToast(`无法预览图片：${errorMessage(error)}`, "error");
  } finally {
    previewLoading.value = false;
  }
}

function closeImagePreview(): void {
  previewImage.value = undefined;
  void nextTick(() => searchInput.value?.focus());
}

function selectOrActivate(entry: LocalClipboardEntry): void {
  // 长按弹菜单后的那次 click 是抬手带出来的，不算选择，避免移动端误触发激活。
  if (longPressFired) {
    longPressFired = false;
    return;
  }
  selectedEntryId.value = entry.id;
  if (isPasteWindow || isMobile.value) emit("activate", entry, true);
}

// —— 条目右键/长按菜单 ——
// 桌面右键原生触发 contextmenu；Android WebView 长按也会合成该事件，触摸
// timer 只是 iOS 等不合成场景的兜底。两者都落在本函数，靠时间戳守卫防双开。
let lastMenuOpenedAt = 0;
let longPressTimer: ReturnType<typeof setTimeout> | undefined;
let longPressFired = false;
let longPressX = 0;
let longPressY = 0;

function openEntryMenu(entry: LocalClipboardEntry, x: number, y: number): void {
  if (isPasteWindow) return;
  const now = Date.now();
  if (now - lastMenuOpenedAt < 600 && menuEntry.value?.id === entry.id) return;
  lastMenuOpenedAt = now;
  selectedEntryId.value = entry.id;
  menuX.value = x;
  menuY.value = y;
  menuEntry.value = entry;
}

function cancelLongPress(): void {
  if (longPressTimer) {
    clearTimeout(longPressTimer);
    longPressTimer = undefined;
  }
}

function handleEntryTouchStart(entry: LocalClipboardEntry, event: TouchEvent): void {
  if (isPasteWindow || event.touches.length !== 1) {
    cancelLongPress();
    return;
  }
  const touch = event.touches[0]!;
  longPressX = touch.clientX;
  longPressY = touch.clientY;
  longPressFired = false;
  longPressTimer = setTimeout(() => {
    longPressTimer = undefined;
    longPressFired = true;
    openEntryMenu(entry, longPressX, longPressY);
  }, 500);
}

function handleEntryTouchMove(event: TouchEvent): void {
  if (!longPressTimer) return;
  const touch = event.touches[0];
  if (!touch) return;
  const dx = touch.clientX - longPressX;
  const dy = touch.clientY - longPressY;
  if (dx * dx + dy * dy > 100) cancelLongPress();
}

function handleEntryTouchEnd(): void {
  cancelLongPress();
}

function closeEntryMenu(): void {
  menuEntry.value = undefined;
}

/// 粘贴窗口用 mousedown 触发粘贴：窗口刚成为 key 窗口且焦点在搜索框时，
/// WebKit 会把第一次点击只用于失焦、吞掉 click 事件，mousedown 不受影响。
function activateOnMouseDown(entry: LocalClipboardEntry): void {
  selectedEntryId.value = entry.id;
  emit("activate", entry, true);
}

function activateSelectedEntry(entry?: LocalClipboardEntry): void {
  if (!entry) return;
  emit("activate", entry, false);
}

/** paste 窗口下载中的条目：再次激活即取消，用 title 告知。 */
function downloadCancelHint(entry: LocalClipboardEntry): string | undefined {
  if (isPasteWindow && props.activatingEntryIds.has(entry.id) && props.downloadProgressByEntryId[entry.id]) {
    return "下载中，再次按 Enter 或点击可取消";
  }
  return undefined;
}

function resetTimeFilter(): void {
  timeFilter.value = "all";
  startDate.value = "";
  endDate.value = "";
}

/**
 * Shared document-keydown hook: the App-level handler delegates here after its
 * own dialogs (settings, setup) had a chance to consume the key. Returns true
 * when the key was handled and should not fall through to window hiding.
 */
function handleKeydown(event: KeyboardEvent): boolean {
  if (previewImage.value) {
    if (event.key === "Escape") {
      event.preventDefault();
      closeImagePreview();
    }
    return true;
  }
  if (event.key === "Enter" && !event.shiftKey) {
    // 焦点在搜索框等输入控件里时，回车属于输入框，不再触发列表条目的
    // 复制/粘贴，避免一次按键同时执行两个操作。
    const target = event.target as HTMLElement | null;
    if (target?.closest("input, textarea, select, [contenteditable]")) return false;
    event.preventDefault();
    activateSelectedEntry(pageEntries.value[Math.max(selectedLocalIndex.value, 0)]);
    return true;
  }
  return false;
}

defineExpose({ handleKeydown, focusSearch, focusSearchInput, currentPage });
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
        <button class="icon-button" type="button" title="上传文件" aria-label="上传文件" :disabled="uploadingFiles" @click="captureFilesFromPicker('file')">
          <LoaderCircle v-if="uploadingFiles" :size="18" class="spin" aria-hidden="true" />
          <FilePlus v-else :size="18" aria-hidden="true" />
        </button>
        <button class="icon-button" type="button" title="上传文件夹" aria-label="上传文件夹" :disabled="uploadingFiles" @click="captureFilesFromPicker('folder')">
          <FolderPlus :size="18" aria-hidden="true" />
        </button>
        <button v-if="isMobile" class="icon-button" type="button" title="设置" aria-label="打开设置" @click="emit('open-settings')">
          <Settings2 :size="19" />
        </button>
      </div>
    </header>

    <section class="toolbar" @mousedown.left="isPasteWindow && startWindowDrag($event)">
      <div v-if="isMobile && importingShare" class="mobile-share-status" role="status" aria-live="polite" aria-atomic="true">
        <LoaderCircle :size="18" class="spin" aria-hidden="true" />
        <span>正在接收系统分享…</span>
      </div>
      <label class="search-field">
        <Search :size="17" aria-hidden="true" />
        <input
          ref="searchInput"
          v-model="query"
          type="search"
          placeholder="搜索剪贴板历史"
          aria-label="搜索剪贴板历史"
          enterkeyhint="search"
          @keydown.enter="!$event.isComposing && commitSearch()"
          @search="commitSearch"
        />
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
          <DeviceFilterControl
            v-if="devices.length"
            v-model="selectedDeviceIds"
            :devices="devices"
          />
        </div>
        <div class="filter-actions">
          <span class="result-summary" :class="{ error: timeRangeError }" :title="filterResultSummary">{{ filterResultSummary }}</span>
        </div>
      </div>
    </section>

    <section ref="historyListElement" class="history-list" aria-label="剪贴板历史" @scroll="closeEntryMenu">
      <div
        v-for="entry in pageEntries"
        :key="entry.id"
        class="history-item"
        :class="{ selected: selectedEntryId === entry.id, 'image-entry': entry.kind === 'image' }"
        role="button"
        :tabindex="activatingEntryIds.has(entry.id) ? -1 : 0"
        :aria-disabled="activatingEntryIds.has(entry.id)"
        :title="downloadCancelHint(entry)"
        @mouseenter="selectedEntryId = entry.id"
        @mousedown.left="isPasteWindow && activateOnMouseDown(entry)"
        @click="selectOrActivate(entry)"
        @keydown.enter.stop="activateSelectedEntry(entry)"
        @keydown.space.prevent.stop="activateSelectedEntry(entry)"
        @contextmenu.prevent="openEntryMenu(entry, $event.clientX, $event.clientY)"
        @touchstart="handleEntryTouchStart(entry, $event)"
        @touchmove="handleEntryTouchMove"
        @touchend="handleEntryTouchEnd"
        @touchcancel="handleEntryTouchEnd"
      >
        <button
          v-if="entry.kind === 'image' && !isPasteWindow"
          class="image-thumbnail"
          type="button"
          :aria-label="`预览${entry.content}`"
          :title="`预览${entry.content}`"
          @click.stop="openImagePreview(entry)"
        >
          <img v-if="thumbnailSource(entry)" :src="thumbnailSource(entry)" :alt="entry.content" loading="lazy" />
          <Image v-else :size="18" aria-hidden="true" />
        </button>
        <span v-else-if="entry.kind === 'image' && thumbnailSource(entry)" class="image-thumbnail" aria-hidden="true">
          <img :src="thumbnailSource(entry)" alt="" loading="lazy" />
        </span>
        <EntryKindIcon
          v-else
          :kind="entry.kind"
          :root-kind="entry.summary.rootKind"
          :loading="activatingEntryIds.has(entry.id)"
        />
        <span class="entry-body">
          <span class="entry-content">{{ entry.content }}</span>
          <span class="entry-meta">
            <Monitor :size="12" /> {{ deviceDisplayName(props.devicesById, entry) }}
            <span>·</span>
            <span :title="formatExactDateTime(entry.createdAt)">{{ formatAge(entry.createdAt) }}</span>
            <template v-if="fileEntrySummary(entry)">
              <span>·</span>
              <span>{{ fileEntrySummary(entry) }}</span>
            </template>
            <template v-if="entryUploadStatus(entry)">
              <span>·</span>
              <span class="upload-status" :class="{ uploaded: entryUploadStatus(entry) === '已上传', uploading: entryUploadStatus(entry)?.startsWith('上传中') }">{{ entryUploadStatus(entry) }}</span>
            </template>
          </span>
          <span v-if="entryDownloadProgress(entry)" class="download-bar entry-download-bar" aria-hidden="true">
            <span class="download-bar-fill" :style="{ width: `${downloadPercentOf(entryDownloadProgress(entry)!)}%` }"></span>
          </span>
        </span>
      </div>

      <div v-if="!manifestTotal" class="empty-state">
        <Search :size="28" />
        <strong>{{ timeRangeError ? "日期区间无效" : timeFilter !== "all" ? "该时间段暂无内容" : "没有匹配内容" }}</strong>
        <span>{{ timeRangeError || (timeFilter !== "all" ? "可以更换时间范围，或清除时间筛选查看全部记录" : isMobile ? "其他设备的内容同步后会显示在这里" : "复制文本后会自动保存到这里") }}</span>
        <button v-if="timeFilter !== 'all'" class="empty-filter-reset" type="button" @click="resetTimeFilter">清除时间筛选</button>
      </div>
    </section>

    <footer class="footer-hint">
      <span v-if="isMobile">点按文本复制，点按文件下载到缓存，长按打开菜单</span>
      <span v-else-if="isPasteWindow">单击记录立即粘贴</span>
      <span v-else>单击选择，右键打开菜单</span>
      <span v-if="!isMobile"><kbd>Enter</kbd> {{ isPasteWindow ? "粘贴" : "复制" }}</span>
      <span v-if="!isMobile"><kbd>Esc</kbd> 关闭</span>
      <PaginationControl
        :page="currentPage"
        :page-count="pageCount"
        :total="manifestTotal"
        @update:page="changePage"
      />
    </footer>

    <EntryContextMenu
      v-if="menuEntry"
      :entry="menuEntry"
      :x="menuX"
      :y="menuY"
      :activating="activatingEntryIds.has(menuEntry.id)"
      :saving-entry-id="savingEntryId"
      :is-mobile="isMobile"
      @activate="emit('activate', $event, false)"
      @save="emit('save', $event)"
      @remove="emit('remove', $event)"
      @close="closeEntryMenu"
    />

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
