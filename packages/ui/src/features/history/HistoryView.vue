<script setup lang="ts">
import { parseLocalDate, validateDateRange, deviceName as deviceDisplayName, fileEntrySummary, uploadStatus as uploadStatusOf } from "../../utils";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { openModalCount } from "@qingfeng346/ui-kit/components/SModal.vue";
import { native } from "../../platform/native";
import {
  ChevronLeft,
  ChevronRight,
  Clipboard,
  Image,
  Search,
  Check,
  ListChecks,
  Trash2,
  Star,
} from "lucide-vue-next";
import TimeFilterControl from "./TimeFilterControl.vue";
import EntryContextMenu from "./EntryContextMenu.vue";
import EntryKindIcon from "../../components/EntryKindIcon.vue";
import ClipboardEntryContent from "../../components/ClipboardEntryContent.vue";
import WorkspacePage from "../../components/WorkspacePage.vue";
import WorkspaceMenu from "../../components/WorkspaceMenu.vue";
import EntryTypeFilter from "./EntryTypeFilter.vue";
import DeviceFilterControl from "./DeviceFilterControl.vue";
import ImagePreview from "./ImagePreview.vue";
import FilePreview from "./FilePreview.vue";
import { useHistoryManifest } from "./useHistoryManifest";
import { setEntriesFavorite } from "./useEntryActions";
import { useHistorySelection } from "./useHistorySelection";
import { usePlatform, showNotification } from "../../platform/runtime";
import { capturingClipboard, captureCurrentClipboard } from "../pending-upload/usePendingUploads";
import type {
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
  quickPaste?: boolean;
  devicesById: Record<string, Device>;
  currentTime: number;
  importingShare: boolean;
  activatingEntryIds: ReadonlySet<string>;
  savingEntryId: string;
  ensureLocalFiles: (entry: LocalClipboardEntry) => Promise<LocalClipboardEntry>;
  removeEntries: (ids: readonly string[]) => Promise<boolean>;
}>();

const emit = defineEmits<{
  activate: [entry: LocalClipboardEntry, viaClick: boolean];
  save: [entry: LocalClipboardEntry];
}>();

const { isMobile } = usePlatform();
const isQuickPaste = computed(() => props.quickPaste === true);

const query = ref("");
const filter = ref<EntryFilter>("all");
const favoriteOnly = ref(false);
const favoritingIds = ref(new Set<string>());
const favoriting = computed(() => favoritingIds.value.size > 0);

async function toggleFavorite(entry: LocalClipboardEntry): Promise<void> {
  await favoriteEntries([entry.id], !entry.isFavorite);
}

async function favoriteEntries(ids: readonly string[], isFavorite: boolean): Promise<void> {
  if (!ids.length || deleting.value || favoriting.value) return;
  for (const id of ids) favoritingIds.value.add(id);
  try { await setEntriesFavorite(ids, isFavorite); }
  finally { for (const id of ids) favoritingIds.value.delete(id); }
}

function favoriteFromMenu(entry: LocalClipboardEntry, isFavorite: boolean): void {
  const ids = selectedIds.value.has(entry.id) && selectedCount.value > 1 ? [...selectedIds.value] : [entry.id];
  void favoriteEntries(ids, isFavorite);
}
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
const deleting = ref(false);
const deleteTargets = ref<string[]>([]);

function focusSearchInputEl(): void {
  searchInput.value?.$el?.querySelector<HTMLInputElement>("input")?.focus({ preventScroll: true });
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
    favoriteOnly: favoriteOnly.value,
    kind: filter.value,
    start: activeTimeRange.value.start,
    end: activeTimeRange.value.end,
    deviceIds: selectedDeviceIds.value,
    page,
  }),
  revision: computed(() => props.revision),
  filterSources: [committedQuery, filter, favoriteOnly, timeFilter, startDate, endDate, selectedDeviceIds],
  // An invalid custom range matches nothing — the backend never sees it, and
  // background revision bumps must not refill the cleared list either.
  canFetch: () => !timeRangeError.value,
  onError: (message) => showNotification({ message: `读取历史失败：${message}`, tone: "error" }),
  listElement: historyListElement,
  getSelectedEntryId: () => selectedEntryId.value,
  setSelectedEntryId: (id) => { selectedEntryId.value = id; },
});

const { selecting, selectedIds, allSelected, start: startSelection, toggle: toggleSelection, toggleAll, clear: clearSelection, setAnchor } = useHistorySelection(computed(() => pageEntries.value.map((entry) => entry.id)));
const selectedCount = computed(() => selectedIds.value.size);
watch(pageEntries, (entries) => {
  if (menuEntry.value && !entries.some((entry) => entry.id === menuEntry.value?.id)) closeEntryMenu();
});
watch([committedQuery, filter, favoriteOnly, timeFilter, startDate, endDate, selectedDeviceIds, currentPage], () => {
  clearSelection();
  closeEntryMenu();
});

function beginSelection(entry?: LocalClipboardEntry): void {
  closeEntryMenu();
  startSelection(entry?.id);
}

function requestSelectedDelete(): void {
  if (!selectedCount.value || deleting.value) return;
  closeEntryMenu();
  deleteTargets.value = [...selectedIds.value];
}

async function confirmDelete(): Promise<void> {
  if (deleting.value || !deleteTargets.value.length) return;
  deleting.value = true;
  try {
    if (await props.removeEntries(deleteTargets.value)) {
      clearSelection();
      deleteTargets.value = [];
    }
    // Keep the dialog and selection after failure so retry never targets new rows.
  } finally { deleting.value = false; }
}

function removeFromMenu(entry: LocalClipboardEntry): void {
  if (selectedIds.value.has(entry.id) && selectedCount.value > 1) requestSelectedDelete();
  else void props.removeEntries([entry.id]);
}

function changeHistoryPage(page: number): void {
  clearSelection();
  closeEntryMenu();
  void changePage(page);
}

function handleSelectionKeys(event: KeyboardEvent): void {
  if (isQuickPaste.value || openModalCount.value || deleting.value) return;
  if (menuEntry.value) {
    // Android's native back dispatches Escape on document rather than the menu.
    if (event.key === "Escape" && event.target === document) {
      event.preventDefault();
      event.stopImmediatePropagation();
      closeEntryMenu();
    }
    return;
  }
  const target = event.target as HTMLElement | null;
  if (selecting.value && event.key === "Escape") {
    event.preventDefault();
    event.stopImmediatePropagation();
    clearSelection();
    return;
  }
  if (target?.closest?.("input, textarea, select, [contenteditable='true']")) return;
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "a" && (selecting.value || historyListElement.value?.contains(target))) {
    event.preventDefault();
    event.stopImmediatePropagation();
    if (!allSelected.value) toggleAll();
  } else if (selecting.value && event.key === "Delete") {
    event.preventDefault();
    event.stopImmediatePropagation();
    requestSelectedDelete();
  }
}

// A mobile browser back action first leaves selection, without leaving the app.
let selectionHistoryActive = false;
watch(selecting, (active) => {
  if (!isMobile.value) return;
  if (active) {
    window.history.pushState({ ...window.history.state, cliproamHistorySelection: true }, "");
    selectionHistoryActive = true;
  } else if (selectionHistoryActive) {
    selectionHistoryActive = false;
    if (window.history.state?.cliproamHistorySelection) window.history.back();
  }
}, { flush: "sync" });
function handleSelectionBack(): void {
  if (!selectionHistoryActive) return;
  if (menuEntry.value || deleteTargets.value.length || deleting.value) {
    window.history.pushState({ ...window.history.state, cliproamHistorySelection: true }, "");
    closeEntryMenu();
    if (!deleting.value) deleteTargets.value = [];
    return;
  }
  selectionHistoryActive = false;
  closeEntryMenu();
  if (!deleting.value) deleteTargets.value = [];
  clearSelection();
}
onMounted(() => {
  document.addEventListener("keydown", handleSelectionKeys, true);
  window.addEventListener("popstate", handleSelectionBack);
});
onBeforeUnmount(() => {
  document.removeEventListener("keydown", handleSelectionKeys, true);
  window.removeEventListener("popstate", handleSelectionBack);
  clearSelection();
});

// An invalid custom range matches nothing — the backend never sees it.
watch(timeRangeError, (error) => { if (error) clearManifest(); });

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
  await native.startWindowDrag();
}

async function focusSearch(): Promise<void> {
  query.value = "";
  committedQuery.value = "";
  if (isQuickPaste.value) {
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
  if (entry.kind === "text") return;
  previewEntry.value = entry;
}

function closeEntryPreview(): void {
  previewEntry.value = undefined;
  void nextTick(focusSearchInputEl);
}

function handleEntryMouseDown(entry: LocalClipboardEntry, event: MouseEvent): void {
  if (!isQuickPaste.value && (selecting.value || event.ctrlKey || event.metaKey || event.shiftKey)) {
    event.preventDefault();
    return;
  }
  if (!isQuickPaste.value) return;
  selectedEntryId.value = entry.id;
  emit("activate", entry, true);
}

function handleEntryClick(entry: LocalClipboardEntry, event: MouseEvent): void {
  if (longPressFired) {
    longPressFired = false;
    return;
  }
  if (!isQuickPaste.value && (selecting.value || event.ctrlKey || event.metaKey || event.shiftKey)) {
    closeEntryMenu();
    toggleSelection(entry.id, event.shiftKey, event.ctrlKey || event.metaKey);
    return;
  }
  setAnchor(entry.id);
  if (!isMobile.value) return;
  if (entry.kind === "text") emit("activate", entry, false);
  else openEntryPreview(entry);
}

function handleEntryDoubleClick(entry: LocalClipboardEntry): void {
  if (selecting.value || isQuickPaste.value || isMobile.value || longPressFired) return;
  if (entry.kind === "text") emit("activate", entry, false);
  else if (!isMobile.value) openEntryPreview(entry);
}

function handleEntryKeydown(entry: LocalClipboardEntry, event: KeyboardEvent): void {
  if (!selecting.value || (event.key !== "Enter" && event.key !== " ")) return;
  event.preventDefault();
  event.stopPropagation();
  toggleSelection(entry.id, event.shiftKey, event.ctrlKey || event.metaKey);
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
  if (isQuickPaste.value || deleting.value) return;
  if (isMobile.value) longPressFired = true;
  const now = Date.now();
  if (now - lastMenuOpenedAt < 600 && menuEntry.value?.id === entry.id) return;
  lastMenuOpenedAt = now;
  selectedEntryId.value = entry.id;
  if (selecting.value && !selectedIds.value.has(entry.id)) startSelection(entry.id);
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
  cancelLongPress();
  if (!isMobile.value || event.touches.length !== 1) {
    return;
  }
  longPressFired = false;
  const touch = event.touches[0]!;
  longPressX = touch.clientX;
  longPressY = touch.clientY;
  longPressTimer = setTimeout(() => {
    longPressTimer = undefined;
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

function resetTimeFilter(): void {
  timeFilter.value = "all";
  startDate.value = "";
  endDate.value = "";
}

defineExpose({ focusSearch, focusSearchInput, currentPage });
</script>

<template>
  <WorkspacePage title="剪贴板历史" layout="history" :data-history-selecting="selecting || undefined">
    <template #menu><WorkspaceMenu /></template>
    <template v-if="isQuickPaste" #header>
      <div class="paste-drag-strip" aria-hidden="true" @mousedown.left="startWindowDrag"></div>
    </template>
    <template v-if="!isQuickPaste" #actions>
      <SButton :disabled="deleting || (!selecting && !pageEntries.length)" @click="selecting ? clearSelection() : beginSelection()">
        <ListChecks :size="15" aria-hidden="true" />
        {{ selecting ? "取消选择" : "选择" }}
      </SButton>
      <SButton
        v-if="!selecting"
        aria-label="读取当前剪贴板"
        title="读取当前剪贴板"
        :aria-busy="capturingClipboard"
        :disabled="capturingClipboard"
        @click="captureCurrentClipboard"
        @keydown.stop
      >
        <span v-if="capturingClipboard" class="s-spinner" aria-hidden="true" />
        <Clipboard v-else :size="15" aria-hidden="true" />
        <span>{{ isMobile ? '读取剪贴板' : '读取当前剪贴板' }}</span>
      </SButton>
    </template>
    <template #toolbar>
    <section class="toolbar" @mousedown.left="isQuickPaste && startWindowDrag($event)">
      <div v-if="isMobile && importingShare" class="mobile-share-status" role="status" aria-live="polite" aria-atomic="true">
        <span class="s-spinner" aria-hidden="true" />
        <span>正在接收系统分享…</span>
      </div>
      <div class="history-search-row">
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
      </div>
      <div class="filter-row" role="group" aria-label="剪贴板筛选">
        <div class="filter-scroll" :class="{ 'has-device-filter': devices.length > 0 }">
          <EntryTypeFilter v-model="filter" :dropdown="isQuickPaste" />
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
          <button
            class="favorite-filter"
            type="button"
            :aria-pressed="favoriteOnly"
            aria-label="仅显示已收藏"
            :title="favoriteOnly ? '显示全部记录' : '仅显示已收藏'"
            @click="favoriteOnly = !favoriteOnly"
          ><Star :size="13" :fill="favoriteOnly ? 'currentColor' : 'none'" aria-hidden="true" /><span>已收藏</span></button>
        </div>
      </div>
      <p v-if="isMobile" class="mobile-history-hint">{{ selecting ? "点击条目勾选，仅选择当前页；返回可退出选择" : "单击文字复制，单击图片或文件预览，长按打开菜单" }}</p>
    </section>
    </template>

    <section ref="historyListElement" class="history-list" aria-label="剪贴板历史" @scroll="closeEntryMenu">
      <div
        v-for="entry in pageEntries"
        :key="entry.id"
        class="history-item"
        :class="{ selected: selecting ? selectedIds.has(entry.id) : selectedEntryId === entry.id, 'image-entry': entry.kind === 'image', 'selection-item': selecting }"
        :role="selecting ? 'checkbox' : 'button'"
        :aria-checked="selecting ? selectedIds.has(entry.id) : undefined"
        :tabindex="deleting || (!selecting && activatingEntryIds.has(entry.id)) ? -1 : 0"
        :aria-disabled="deleting || (!selecting && activatingEntryIds.has(entry.id))"
        @mouseenter="!isMobile && !selecting && (selectedEntryId = entry.id)"
        @mousedown.left="handleEntryMouseDown(entry, $event)"
        @click="handleEntryClick(entry, $event)"
        @keydown="handleEntryKeydown(entry, $event)"
        @dblclick.stop="handleEntryDoubleClick(entry)"
        @contextmenu.prevent="openEntryMenu(entry, $event.clientX, $event.clientY)"
        @touchstart="handleEntryTouchStart(entry, $event)"
        @touchmove="handleEntryTouchMove"
        @touchend="handleEntryTouchEnd"
        @touchcancel="handleEntryTouchEnd"
      >
        <span v-if="selecting" class="entry-selection-box" :class="{ checked: selectedIds.has(entry.id) }" aria-hidden="true">
          <Check v-if="selectedIds.has(entry.id)" :size="16" />
        </span>
        <button
          v-else-if="entry.kind === 'image' && isQuickPaste"
          class="image-thumbnail"
          type="button"
          :aria-label="`预览${entry.content}`"
          :title="`预览${entry.content}`"
          @click.stop="openEntryPreview(entry)"
          @mousedown.stop
          @dblclick.stop
          @keydown.stop
          @touchstart.stop
        >
          <img v-if="thumbnailSource(entry)" :src="thumbnailSource(entry)" :alt="entry.content" loading="lazy" />
          <Image v-else :size="18" aria-hidden="true" />
        </button>
        <span v-else-if="entry.kind === 'image'" class="image-thumbnail" aria-hidden="true">
          <img v-if="thumbnailSource(entry)" :src="thumbnailSource(entry)" alt="" loading="lazy" />
          <Image v-else :size="18" />
        </span>
        <button
          v-else-if="entry.kind === 'files' && isQuickPaste"
          class="file-preview-trigger"
          type="button"
          :aria-label="`预览${entry.content}`"
          :title="`预览${entry.content}`"
          @click.stop="openEntryPreview(entry)"
          @mousedown.stop
          @dblclick.stop
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
        <button
          v-if="!selecting"
          class="entry-favorite"
          type="button"
          :class="{ active: entry.isFavorite }"
          :aria-label="entry.isFavorite ? '取消收藏' : '收藏'"
          :title="entry.isFavorite ? '取消收藏' : '收藏'"
          :aria-pressed="!!entry.isFavorite"
          :aria-busy="favoritingIds.has(entry.id)"
          :disabled="favoriting"
          @click.stop="toggleFavorite(entry)"
          @mousedown.stop
          @dblclick.stop
          @keydown.stop
          @touchstart.stop
          @touchend.stop
        ><Star :size="17" :fill="entry.isFavorite ? 'currentColor' : 'none'" aria-hidden="true" /></button>
      </div>

      <SEmpty v-if="!manifestTotal" class="empty-state">
        <Search :size="28" aria-hidden="true" />
        <strong>{{ timeRangeError ? "日期区间无效" : favoriteOnly ? "没有匹配的收藏" : timeFilter !== "all" ? "该时间段暂无内容" : "没有匹配内容" }}</strong>
        <span>{{ timeRangeError || (favoriteOnly ? "点击条目的星标收藏，也可以调整搜索和筛选条件" : timeFilter !== "all" ? "可以更换时间范围，或清除时间筛选查看全部记录" : isMobile ? "其他设备的内容同步后会显示在这里" : "复制文本后会自动保存到这里") }}</span>
        <SButton v-if="timeFilter !== 'all'" class="empty-filter-reset" size="small" @click="resetTimeFilter">清除时间筛选</SButton>
      </SEmpty>
    </section>

    <template #footer>
      <div v-if="selecting" class="history-selection-actions" role="toolbar" aria-label="历史多选操作">
        <span role="status" aria-live="polite">已选 {{ selectedCount }} 条</span>
        <SButton :disabled="deleting || !pageEntries.length" @click="toggleAll">{{ allSelected ? "取消全选" : "本页全选" }}</SButton>
        <SButton :disabled="deleting || favoriting || !selectedCount" @click="favoriteEntries([...selectedIds], true)">
          <Star :size="15" aria-hidden="true" />收藏
        </SButton>
        <SButton :disabled="deleting || favoriting || !selectedCount" @click="favoriteEntries([...selectedIds], false)">取消收藏</SButton>
        <SButton class="history-selection-delete" type="error" :disabled="deleting || !selectedCount" @click="requestSelectedDelete">
          <Trash2 :size="15" aria-hidden="true" />
          删除 {{ selectedCount }} 条
        </SButton>
      </div>
      <template v-else>
      <span v-if="isQuickPaste">单击条目粘贴，点击图片或文件图标预览</span>
      <span v-else-if="!isMobile">双击图片或文件预览，双击文字复制，右键打开菜单</span>
      <span v-if="!isMobile"><kbd>Esc</kbd> 关闭</span>
      <nav class="pagination-control" aria-label="历史记录分页">
        <button
          class="pagination-button"
          type="button"
          title="上一页"
          aria-label="上一页"
          :disabled="currentPage <= 1"
          @click="changeHistoryPage(currentPage - 1)"
        >
          <ChevronLeft :size="15" aria-hidden="true" />
        </button>
        <span class="pagination-status" aria-live="polite">共 {{ manifestTotal }} 条 · {{ isMobile ? `${currentPage} / ${pageCount}` : `第 ${currentPage} / ${pageCount} 页` }}</span>
        <button
          class="pagination-button"
          type="button"
          title="下一页"
          aria-label="下一页"
          :disabled="currentPage >= pageCount"
          @click="changeHistoryPage(currentPage + 1)"
        >
          <ChevronRight :size="15" aria-hidden="true" />
        </button>
      </nav>
      </template>
    </template>

    <EntryContextMenu
      v-if="menuEntry"
      :key="menuEntry.id"
      :entry="menuEntry"
      :favoriting="favoriting"
      @favorite="favoriteFromMenu"
      :x="menuX"
      :y="menuY"
      :activating="activatingEntryIds.has(menuEntry.id)"
      :saving-entry-id="savingEntryId"
      :is-mobile="isMobile"
      :selected-count="selectedIds.has(menuEntry.id) ? selectedCount : 0"
      @activate="emit('activate', $event, false)"
      @save="emit('save', $event)"
      @remove="removeFromMenu"
      @select="beginSelection"
      @close="closeEntryMenu"
    />

    <SModal v-if="deleteTargets.length" :show="true" :title="`删除 ${deleteTargets.length} 条历史？`" width="420px"
      :closable="!deleting" :mask-closable="!deleting" :close-on-esc="!deleting"
      @update:show="!deleting && (deleteTargets = [])">
      <p>所选历史会从当前账号的所有设备同步删除，此操作无法撤销。</p>
      <template #footer>
        <SButton :disabled="deleting" @click="deleteTargets = []">取消</SButton>
        <SButton type="error" :loading="deleting" @click="confirmDelete">确认删除 {{ deleteTargets.length }} 条</SButton>
      </template>
    </SModal>

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
.toolbar { container: history-toolbar / inline-size; }
.favorite-filter { display: inline-flex; flex: 0 0 auto; align-items: center; justify-content: center; gap: 5px; height: 27px; margin-left: 5px; padding: 0 9px; border: 1px solid rgba(148, 163, 184, 0.18); border-radius: 6px; background: rgba(255, 255, 255, 0.04); color: #cbd5e1; font: inherit; font-size: 11px; white-space: nowrap; cursor: pointer; }
.favorite-filter[aria-pressed="true"] { color: var(--sui-primary); background: var(--sui-bg-active); border-color: var(--sui-primary); }
.entry-favorite { display: grid; place-items: center; width: 32px; height: 32px; padding: 0; border: 0; border-radius: 6px; background: transparent; color: var(--color-secondary-text); cursor: pointer; }
.entry-favorite.active { color: var(--sui-primary); }
.entry-favorite:hover, .favorite-filter:hover { background: var(--sui-bg-hover); }
.entry-favorite:focus-visible, .favorite-filter:focus-visible { outline: 2px solid var(--sui-primary); outline-offset: 2px; }
.entry-favorite:disabled { opacity: 0.5; cursor: wait; }
:global(.mobile-app .history-item), :global(.mobile-app .history-item.image-entry) { grid-template-columns: 36px minmax(0, 1fr) auto; }
:global(.mobile-app .filter-scroll .favorite-filter) { width: 44px; height: 44px; margin-left: 0; padding-inline: 9px; border-radius: 5px; }
:global(.mobile-app .favorite-filter span) { display: none; }
:global(.mobile-app .favorite-filter svg) { width: 17px; height: 17px; }
:global(.mobile-app .entry-favorite) { min-width: 44px; min-height: 44px; }
.file-preview-trigger { display: grid; place-items: center; padding: 0; border: 0; border-radius: 5px; background: transparent; cursor: pointer; }
.file-preview-trigger:hover { background: var(--sui-bg-hover); }
.selection-item { user-select: none; -webkit-user-select: none; -webkit-touch-callout: none; }
.entry-selection-box { display: grid; place-items: center; justify-self: center; width: 22px; height: 22px; border: 1px solid var(--sui-border-strong); border-radius: 5px; }
.entry-selection-box.checked { color: var(--sui-primary); border-color: var(--sui-primary); background: var(--sui-bg-active); }
.history-selection-actions { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; width: 100%; min-width: 0; min-height: 52px; color: var(--color-foreground); font-size: 13px; }
.history-selection-actions :deep(.s-button) { flex-shrink: 0; }
.history-selection-delete { margin-left: auto; }
@container history-toolbar (max-width: 650px) {
  .filter-scroll { display: flex; gap: 6px; overflow: visible; }
  .filter-scroll :deep(.entry-type-select),
  .filter-scroll :deep(.time-filter-control),
  .filter-scroll :deep(.device-filter-control) { flex: 1 1 0; width: 0; min-width: 0; margin-left: 0; padding-left: 0; border-left: 0; }
  .filter-scroll :deep(.time-filter-label), .filter-scroll :deep(.device-filter-label) { display: none; }
  .filter-scroll :deep(.time-filter-control .time-filter-trigger),
  .filter-scroll :deep(.device-filter-control .device-filter-trigger) { flex: 1 1 auto; width: 100%; min-width: 0; max-width: none; }
  .favorite-filter { margin-left: 0; }
}
@container history-toolbar (max-width: 460px) {
  .favorite-filter { flex: 0 0 32px; width: 32px; height: 27px; padding: 0; border-radius: 6px; }
  .favorite-filter span { display: none; }
}
:global(.mobile-app .filter-scroll) { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) 44px; }
:global(.mobile-app .filter-scroll.has-device-filter) { grid-template-columns: repeat(3, minmax(0, 1fr)) 44px; }
:global(.mobile-app .filter-scroll .time-filter-control),
:global(.mobile-app .filter-scroll .device-filter-control) { width: auto; }
:global(.mobile-app .filter-scroll .time-filter-control .time-filter-trigger),
:global(.mobile-app .filter-scroll .device-filter-control .device-filter-trigger) { width: 100%; min-width: 0; max-width: none; }
@media (max-width: 640px) {
  .file-preview-trigger { min-width: 44px; min-height: 44px; }
  .history-selection-actions :deep(.s-button) { min-height: 44px; }
}
</style>
