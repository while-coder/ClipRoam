<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import {
  Clipboard,
  Image,
  Search,
} from "lucide-vue-next";
import TimeFilterControl from "./TimeFilterControl.vue";
import EntryContextMenu from "./EntryContextMenu.vue";
import EntryKindIcon from "../../components/EntryKindIcon.vue";
import ClipboardEntryContent from "../../components/ClipboardEntryContent.vue";
import WorkspacePage from "../../components/WorkspacePage.vue";
import EntryTypeFilter from "./EntryTypeFilter.vue";
import DeviceFilterControl from "./DeviceFilterControl.vue";
import PaginationControl from "./PaginationControl.vue";
import ImagePreview from "./ImagePreview.vue";
import FilePreview from "./FilePreview.vue";
import { useHistoryManifest } from "./useHistoryManifest";
import { isPasteWindow, usePlatform } from "../../composables/usePlatform";
import { showToast } from "../toast/useToast";
import { capturingClipboard, captureCurrentClipboard } from "../pending-upload/useClipboardCapture";
import {
  parseLocalDate,
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
  EntriesManifestFilter,
  EntriesManifestPage,
  EntryFilter,
  LocalClipboardEntry,
  TimeFilter,
} from "../../types";

const props = defineProps<{
  /** Fetch a server page, backfilling missing details into the local cache. */
  fetchHistoryPage: (
    filter: EntriesManifestFilter,
  ) => Promise<EntriesManifestPage>;
  /** Bumped whenever the history may have changed in the background. */
  revision: number;
  devicesById: Record<string, Device>;
  currentTime: number;
  importingShare: boolean;
  activatingEntryIds: ReadonlySet<string>;
  savingEntryId: string;
  ensureLocalFiles: (entry: LocalClipboardEntry) => Promise<LocalClipboardEntry>;
}>();

const emit = defineEmits<{
  activate: [entry: LocalClipboardEntry, viaClick: boolean];
  remove: [entry: ClipboardEntry];
  save: [entry: LocalClipboardEntry];
}>();

const { isMobile } = usePlatform();

const query = ref("");
const filter = ref<EntryFilter>("all");
const timeFilter = ref<TimeFilter>("all");
const startDate = ref("");
const endDate = ref("");
const selectedEntryId = ref("");
const menuEntry = ref<LocalClipboardEntry>();
const menuX = ref(0);
const menuY = ref(0);
const previewEntry = ref<LocalClipboardEntry>();
// SInput 不 expose 内部 input：实例根节点上 querySelector 一次拿原生元素。
const searchInput = ref<{ $el?: Element }>();
const historyListElement = ref<HTMLElement>();

function focusSearchInputEl(): void {
  searchInput.value?.$el?.querySelector<HTMLInputElement>("input")?.focus();
}

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
    void fetchPage(1, true);
    return;
  }
  committedQuery.value = query.value.trim();
}

// SInput clearable 的 × 只发 update:value("")，空值即显式清空搜索并重查。
function onQueryUpdate(value: string): void {
  if (!value) commitSearch();
}
onBeforeUnmount(cancelLongPress);

const devices = computed(() => Object.values(props.devicesById));
const selectedDeviceIds = ref<string[]>([]);

const {
  page: currentPage,
  total: manifestTotal,
  pageCount,
  entries: pageEntries,
  fetch: fetchPage,
  clear: clearManifest,
  changePage,
} = useHistoryManifest({
  fetchHistoryPage: props.fetchHistoryPage,
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

function entryUploadStatus(entry: LocalClipboardEntry): string | undefined {
  return uploadStatusOf(entry);
}

/** upload-status 文案 → STag 色相：成功绿 / 上传蓝 / 进行中灰 / 待传警示黄。 */
function uploadStatusTagType(status: string | undefined): string {
  if (status === "已上传") return "success";
  if (status?.startsWith("上传中")) return "info";
  if (status?.startsWith("计算中")) return "default";
  return "warning";
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
  await fetchPage(1, true);
  selectedEntryId.value = pageEntries.value[0]?.id ?? "";
  await nextTick();
  focusSearchInputEl();
}

/** 只把焦点还给搜索框；不清搜索词、不重拉列表（关设置弹窗等场景用）。 */
function focusSearchInput(): void {
  focusSearchInputEl();
}

function openEntryPreview(entry: LocalClipboardEntry): void {
  if (isPasteWindow || entry.kind === "text") return;
  previewEntry.value = entry;
}

function closeEntryPreview(): void {
  previewEntry.value = undefined;
  void nextTick(focusSearchInputEl);
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

function resetTimeFilter(): void {
  timeFilter.value = "all";
  startDate.value = "";
  endDate.value = "";
}

/**
 * Shared document-keydown hook: the App-level handler delegates here after its
 * own dialogs (settings, setup) and the ui-kit modal guard had their chance.
 * Returns true when the key was handled and should not fall through to window
 * hiding. Image preview is an SModal now — Esc/Tab/Enter stay with its stack.
 */
function handleKeydown(event: KeyboardEvent): boolean {
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
  <WorkspacePage title="剪贴板历史" layout="history">
    <template v-if="isPasteWindow" #header>
      <div class="paste-drag-strip" aria-hidden="true" @mousedown.left="startWindowDrag"></div>
    </template>
    <template v-if="!isPasteWindow" #actions>
      <button
        class="capture-clipboard-button"
        type="button"
        aria-label="读取当前剪贴板"
        title="读取当前剪贴板"
        :aria-busy="capturingClipboard"
        :disabled="capturingClipboard"
        @click="captureCurrentClipboard"
        @keydown.stop
      >
        <span v-if="capturingClipboard" class="s-spinner" aria-hidden="true" />
        <Clipboard v-else :size="18" aria-hidden="true" />
        <span>读取当前剪贴板</span>
      </button>
    </template>
    <template #toolbar>
    <section class="toolbar" @mousedown.left="isPasteWindow && startWindowDrag($event)">
      <div v-if="isMobile && importingShare" class="mobile-share-status" role="status" aria-live="polite" aria-atomic="true">
        <span class="s-spinner" aria-hidden="true" />
        <span>正在接收系统分享…</span>
      </div>
      <label class="search-field">
        <Search :size="17" aria-hidden="true" />
        <SInput
          ref="searchInput"
          v-model:value="query"
          type="text"
          placeholder="搜索剪贴板历史"
          aria-label="搜索剪贴板历史"
          enterkeyhint="search"
          clearable
          @keydown.enter="!$event.isComposing && commitSearch()"
          @update:value="onQueryUpdate"
        />
        <kbd>Enter</kbd>
      </label>
      <div class="filter-row" role="group" aria-label="剪贴板筛选">
        <div class="filter-scroll">
          <EntryTypeFilter v-model="filter" />
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
      <p v-if="isMobile" class="mobile-history-hint">点按文本复制，点按文件下载，长按打开菜单</p>
    </section>
    </template>

    <section ref="historyListElement" class="history-list" aria-label="剪贴板历史" @scroll="closeEntryMenu">
      <div
        v-for="entry in pageEntries"
        :key="entry.id"
        class="history-item"
        :class="{ selected: selectedEntryId === entry.id, 'image-entry': entry.kind === 'image' }"
        role="button"
        :tabindex="activatingEntryIds.has(entry.id) ? -1 : 0"
        :aria-disabled="activatingEntryIds.has(entry.id)"
        @mouseenter="selectedEntryId = entry.id"
        @mousedown.left="isPasteWindow && activateOnMouseDown(entry)"
        @click="selectOrActivate(entry)"
        @dblclick.stop="openEntryPreview(entry)"
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
          @click.stop="openEntryPreview(entry)"
          @keydown.stop
          @touchstart.stop
        >
          <img v-if="thumbnailSource(entry)" :src="thumbnailSource(entry)" :alt="entry.content" loading="lazy" />
          <Image v-else :size="18" aria-hidden="true" />
        </button>
        <span v-else-if="entry.kind === 'image' && thumbnailSource(entry)" class="image-thumbnail" aria-hidden="true">
          <img :src="thumbnailSource(entry)" alt="" loading="lazy" />
        </span>
        <button
          v-else-if="entry.kind === 'files' && !isPasteWindow"
          class="file-preview-trigger"
          type="button"
          :aria-label="`预览${entry.content}`"
          :title="`预览${entry.content}`"
          @click.stop="openEntryPreview(entry)"
          @keydown.stop
          @touchstart.stop
        >
          <EntryKindIcon :kind="entry.kind" :root-kind="entry.summary.rootKind" :loading="activatingEntryIds.has(entry.id)" />
        </button>
        <EntryKindIcon
          v-else
          :kind="entry.kind"
          :root-kind="entry.summary.rootKind"
          :loading="activatingEntryIds.has(entry.id)"
        />
        <ClipboardEntryContent :entry="entry" :device-name="deviceDisplayName(devicesById, entry)" :current-time="currentTime">
            <template v-if="fileEntrySummary(entry)">
              <span>·</span>
              <span>{{ fileEntrySummary(entry) }}</span>
            </template>
            <template v-if="entryUploadStatus(entry)">
              <span>·</span>
              <STag size="small" :type="uploadStatusTagType(entryUploadStatus(entry))">{{ entryUploadStatus(entry) }}</STag>
            </template>
        </ClipboardEntryContent>
      </div>

      <SEmpty v-if="!manifestTotal" class="empty-state">
        <Search :size="28" aria-hidden="true" />
        <strong>{{ timeRangeError ? "日期区间无效" : timeFilter !== "all" ? "该时间段暂无内容" : "没有匹配内容" }}</strong>
        <span>{{ timeRangeError || (timeFilter !== "all" ? "可以更换时间范围，或清除时间筛选查看全部记录" : isMobile ? "其他设备的内容同步后会显示在这里" : "复制文本后会自动保存到这里") }}</span>
        <SButton v-if="timeFilter !== 'all'" class="empty-filter-reset" size="small" @click="resetTimeFilter">清除时间筛选</SButton>
      </SEmpty>
    </section>

    <template #footer>
      <span v-if="!isMobile && isPasteWindow">单击记录立即粘贴</span>
      <span v-else-if="!isMobile">双击预览图片或文件，右键打开菜单</span>
      <span v-if="!isMobile"><kbd>Enter</kbd> {{ isPasteWindow ? "粘贴" : "复制" }}</span>
      <span v-if="!isMobile"><kbd>Esc</kbd> 关闭</span>
      <PaginationControl
        :page="currentPage"
        :page-count="pageCount"
        :total="manifestTotal"
        @update:page="changePage"
      />
    </template>

    <EntryContextMenu
      v-if="menuEntry"
      :entry="menuEntry"
      :x="menuX"
      :y="menuY"
      :activating="activatingEntryIds.has(menuEntry.id)"
      :saving-entry-id="savingEntryId"
      :is-mobile="isMobile"
      @activate="emit('activate', $event, false)"
      @preview="openEntryPreview"
      @save="emit('save', $event)"
      @remove="emit('remove', $event)"
      @close="closeEntryMenu"
    />

    <ImagePreview
      v-if="previewEntry?.kind === 'image'"
      :key="previewEntry.id"
      :entry="previewEntry"
      :ensure-local-files="ensureLocalFiles"
      @close="closeEntryPreview"
    />
    <FilePreview
      v-else-if="previewEntry?.kind === 'files'"
      :key="previewEntry.id"
      :entry="previewEntry"
      @close="closeEntryPreview"
    />
  </WorkspacePage>
</template>

<style scoped>
.capture-clipboard-button {
  display: inline-flex; align-items: center; justify-content: center; gap: 6px;
  align-self: center; flex-shrink: 0; min-width: 44px; min-height: 44px; padding: 0 10px;
  color: var(--color-secondary-text); background: transparent; border: 0; border-radius: 8px;
  font: inherit; font-size: 13px; white-space: nowrap; cursor: pointer; touch-action: manipulation;
}
.capture-clipboard-button:hover:not(:disabled) { color: var(--color-foreground); background: var(--sui-bg-hover); }
.capture-clipboard-button:active:not(:disabled) { background: var(--sui-bg-active); }
.capture-clipboard-button:focus-visible { outline: 2px solid var(--color-accent); outline-offset: 2px; }
.capture-clipboard-button:disabled { opacity: 0.5; cursor: default; }
.file-preview-trigger { display: grid; place-items: center; padding: 0; border: 0; border-radius: 5px; background: transparent; cursor: pointer; }
.file-preview-trigger:hover { background: var(--sui-bg-hover); }
@media (max-width: 640px) {
  .file-preview-trigger { min-width: 44px; min-height: 44px; }
}
</style>
