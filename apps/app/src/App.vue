<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { addPluginListener, invoke, type PluginListener } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { cursorPosition, getCurrentWindow, monitorFromPoint, PhysicalPosition, type Monitor } from "@tauri-apps/api/window";
import { UpdaterDialog } from "@while-coder/tauri-updater-vue";
import type {
  ClipboardEntry,
  ClipboardManifestEntry,
} from "@cliproam/protocol";
import { DEFAULT_AUTO_RECEIVE_CLIPBOARD, DEFAULT_AUTO_UPLOAD_LIMIT_MB, DEFAULT_SERVER_PROTOCOL } from "@cliproam/protocol";
import {
  Clipboard,
  Cloud,
  CloudOff,
  CloudUpload,
  LoaderCircle,
  Settings2,
} from "lucide-vue-next";
import {
  SyncClient,
  authenticateAccount,
  getServerUrls,
  testSyncConnection,
} from "./features/sync/syncClient";
import { mapWithConcurrency, TRANSFER_CONCURRENCY } from "./features/sync/concurrency";
import {
  disposeQuickPasteShortcut,
  initializeQuickPasteShortcut,
  quickPasteShortcutStatus,
} from "./features/quick-paste/quickPasteShortcut";
import { useUpdater } from "./features/settings/useUpdater";
import { initSettings } from "./features/settings/useSettings";
import { closeSettings, openSettings, settingsVisible } from "./features/settings/useSettings";
import SettingsDialog from "./features/settings/SettingsDialog.vue";
import HistoryView from "./features/clipboard-history/HistoryView.vue";
import PendingSyncView from "./features/pending-sync/PendingSyncView.vue";
import SetupWizard from "./features/setup/SetupWizard.vue";
import type { SetupDraft } from "./features/setup/SetupWizard.vue";
import ToastLayer from "./features/toast/ToastLayer.vue";
import { disposeToast, showToast, startToastWindowListener } from "./features/toast/useToast";
import {
  BROWSER_CONFIG_KEY,
  DEFAULT_SERVER_ADDRESS,
  DESKTOP_CAPABILITIES,
  EMPTY_SUMMARY,
  PAGE_SIZE,
} from "./utils/constants";
import { canSaveEntry, isHashing } from "./utils/entry";
import { errorMessage } from "./utils/error";
import { isToastWindow, isPasteWindow, runningInTauri, usePlatform } from "./composables/usePlatform";
import type {
  Device,
  DownloadProgress,
  EntriesManifestFilter,
  EntriesManifestPage,
  LocalClipboardEntry,
  MissingFile,
  PlatformCapabilities,
  SavePreparation,
  ShareImportSummary,
  ShareReceiverEvent,
  SyncConfig,
  UploadProgress,
  VirtualFileRequest,
} from "./types";

const { platformCapabilities, isMobile, setPlatformCapabilities } = usePlatform();

const { initUpdaterVersion } = useUpdater();

/**
 * The durable upload queue's rows: captured offline or waiting to publish.
 * Served by Rust's `list_pending_entries` (every queue row as an entry-shaped
 * view with a temporary `p{seq}` id) — never derived from a whole-history read.
 * Details load only while the pending-sync view is open; refresh bursts
 * elsewhere carry the O(1) `count_pending_entries` instead.
 */
const pendingEntries = ref<LocalClipboardEntry[]>([]);
const pendingCount = ref(0);
/** Total entries across every filter; backs the clear-history affordance. */
const totalEntryCount = ref(0);
/** Bumped whenever the history may have changed; the history view refetches its page on it. */
const historyRevision = ref(0);
const syncedEntryIds = ref(new Set<string>());
const activeView = ref<"history" | "pending-sync">("history");
const devicesById = ref<Record<string, Device>>({
  browser: { id: "browser", name: "浏览器预览", platform: "browser", osVersion: "未知" },
});
const currentTime = ref(Date.now());
const connected = ref(false);
const syncEnabled = ref(false);
const initializing = ref(true);
const setupVisible = ref(false);
const hasSavedSyncConfig = ref(false);
const setupError = ref("");
const testingConnection = ref(false);
const currentUsername = ref("");
const importingShare = ref(false);
const activatingEntryId = ref("");
const uploadProgressByEntryId = ref<Record<string, UploadProgress>>({});
const downloadProgressByEntryId = ref<Record<string, DownloadProgress>>({});
/**
 * Contents the server pool holds, refreshed live from `/files/query` on every
 * history read and `file.available` push. Deliberately never persisted — it is
 * server state that would go stale — so `undefined` (no sync client) means the
 * upload status is unknown and stays hidden instead of misreporting.
 */
const storedFileIds = ref<Set<string> | undefined>(undefined);
const savingEntryId = ref("");
const historyView = ref<InstanceType<typeof HistoryView>>();
const setupWizard = ref<InstanceType<typeof SetupWizard>>();
let activeSyncConfig: SyncConfig | undefined;
let syncClient: SyncClient | undefined;
let unlisteners: UnlistenFn[] = [];
let ageRefreshTimer: number | undefined;
let shareReceiverListener: PluginListener | undefined;
let localClipboardRevision = 0;
let remoteActivationRevision = 0;

// 设置弹窗（useSettings 单例）通过 bridge 触达同步引擎；App.vue 持有引擎状态。
initSettings({
  getActiveConfig: () => activeSyncConfig,
  setActiveConfig: (config) => { activeSyncConfig = config; },
  getUsername: () => currentUsername.value,
  setUsername: (name) => { currentUsername.value = name; },
  persistSyncConfig,
  startSync,
  disconnect: (syncEnabledAfter) => {
    stopSyncClient(syncEnabledAfter);
  },
  uploadNowEligibleEntries,
  openSetup: ({ config, message, focus }) => {
    if (message !== undefined) setupError.value = message;
    setupVisible.value = true;
    // SetupWizard 挂载后才持有表单状态，setFields/focus 须等下一个 tick。
    void nextTick(() => {
      if (config) setupWizard.value?.setFields(config);
      if (focus === "password") setupWizard.value?.focusPasswordInput();
      else if (focus === "server") setupWizard.value?.focusServerInput();
    });
  },
  focusSearch,
});

/** Tears the sync client down; the optional argument updates the sync switch with it. */
function stopSyncClient(syncEnabledAfter?: boolean): void {
  syncClient?.stop();
  syncClient = undefined;
  connected.value = false;
  if (syncEnabledAfter !== undefined) syncEnabled.value = syncEnabledAfter;
}

const connectionStatus = computed(() => {
  if (connected.value) {
    return {
      label: "已连接",
      title: "已连接到同步服务器",
      tone: "online",
    };
  }
  if (syncEnabled.value) {
    return {
      label: "与服务器断开连接",
      title: "正在等待同步服务器重新连接",
      tone: "disconnected",
    };
  }
  return {
    label: "脱机状态",
    title: "仅使用本地剪贴板历史",
    tone: "offline",
  };
});

const demoEntries: LocalClipboardEntry[] = [
  {
    id: "welcome",
    kind: "text",
    content: "ClipRoam 已准备好。复制一段文字，它会自动出现在这里。",
    sourceDeviceId: "browser",
    createdAt: new Date().toISOString(),
    summary: EMPTY_SUMMARY,
  },
];

/**
 * Browser-preview only: the stand-in list `clientManifest` filters, standing in
 * for the durable history that lives in SQLite when running inside Tauri.
 */
const previewEntries = ref<LocalClipboardEntry[]>(demoEntries);

/**
 * Browser-preview stand-in for `list_entries_manifest`: the same filters the
 * Rust command applies, over the local demo list.
 */
function clientManifest(filter: EntriesManifestFilter, deviceNames: Record<string, string>): EntriesManifestPage {
  const needle = (filter.query ?? "").trim().toLowerCase();
  const matched = previewEntries.value.filter((entry) => {
    if (filter.kind && filter.kind !== "all" && entry.kind !== filter.kind) return false;
    if (needle) {
      const deviceLabel = (deviceNames[entry.sourceDeviceId] ?? "未知设备").toLowerCase();
      const matches = entry.content.toLowerCase().includes(needle) || deviceLabel.includes(needle);
      if (!matches) return false;
    }
    const createdAt = new Date(entry.createdAt).getTime();
    if (filter.start !== undefined && createdAt < filter.start) return false;
    if (filter.end !== undefined && createdAt > filter.end) return false;
    return true;
  });
  const page = filter.page;
  return {
    total: matched.length,
    allTotal: previewEntries.value.length,
    entries: page ? matched.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE) : matched,
  };
}

async function fetchManifest(
  filter: EntriesManifestFilter,
  deviceNames: Record<string, string>,
): Promise<EntriesManifestPage> {
  if (!runningInTauri) {
    const page = clientManifest(filter, deviceNames);
    totalEntryCount.value = previewEntries.value.length;
    return page;
  }
  const page = await invoke<EntriesManifestPage>("list_entries_manifest", { filter, deviceNames });
  totalEntryCount.value = page.allTotal;
  return page;
}

/** The pending-sync list re-queries Rust; the browser preview derives it. */
async function refreshPendingEntries(): Promise<void> {
  if (!runningInTauri) {
    pendingEntries.value = previewEntries.value.filter((entry) => !isEntrySynced(entry));
    pendingCount.value = pendingEntries.value.length;
    return;
  }
  try {
    pendingEntries.value = await invoke<LocalClipboardEntry[]>("list_pending_entries");
    pendingCount.value = pendingEntries.value.length;
  } catch (error) {
    showToast(`待同步记录读取失败：${errorMessage(error)}`, "error");
  }
}

/**
 * Sidebar badge only: an O(1) count, so refresh bursts outside the pending
 * view never haul the queue rows across the IPC boundary.
 */
async function refreshPendingCount(): Promise<void> {
  if (!runningInTauri) {
    pendingCount.value = previewEntries.value.filter((entry) => !isEntrySynced(entry)).length;
    return;
  }
  try {
    pendingCount.value = await invoke<number>("count_pending_entries");
  } catch {
    // The badge is auxiliary; a failed refresh keeps the previous value.
  }
}

watch(activeView, (view) => {
  if (view === "pending-sync") void refreshPendingEntries();
});

let refreshTimer: number | undefined;

/**
 * Background events (captures, remote upserts, file availability) arrive in
 * bursts; each one only invalidates views. A burst coalesces into one pass:
 * the history view refetches its current page on the revision bump, while the
 * pending badge (and, when its view is open, the queue details) re-query
 * Rust-side. Nothing reads whole history.
 */
function refreshHistory(): void {
  if (refreshTimer !== undefined) return;
  refreshTimer = window.setTimeout(() => {
    refreshTimer = undefined;
    historyRevision.value += 1;
    void refreshPendingCount();
    if (activeView.value === "pending-sync") void refreshPendingEntries();
    void refreshStoredFileIds();
  }, 200);
}

/**
 * Upload status is derived, never stored: the content ids come from the
 * durable history's extras (computed Rust-side) and the pool is asked live
 * which ones it holds. Contents already known stored skip the query, so a
 * steady state costs nothing; a failure clears the set so the status text
 * hides instead of misreporting.
 */
async function refreshStoredFileIds(): Promise<void> {
  const client = syncClient;
  if (!client) {
    storedFileIds.value = undefined;
    return;
  }
  try {
    const fileIds = await invoke<string[]>("history_file_ids");
    const unchecked = fileIds.filter((fileId) => !storedFileIds.value?.has(fileId));
    if (!unchecked.length) return;
    const statuses = await client.fetchFileStatuses(unchecked);
    const next = new Set(storedFileIds.value);
    for (const file of statuses) {
      if (file.stored) next.add(file.fileId);
    }
    storedFileIds.value = next;
  } catch {
    storedFileIds.value = undefined;
  }
}

const pendingRemoteUpserts = new Map<string, ClipboardEntry>();
let remoteUpsertFlush: Promise<void> | undefined;

/**
 * Remote entry echoes arrive one per published entry, but each write rewrites
 * the durable history. Queue them so a burst becomes a single batch command.
 */
function queueRemoteUpsert(entry: ClipboardEntry): Promise<void> {
  pendingRemoteUpserts.set(entry.id, entry);
  if (remoteUpsertFlush) return remoteUpsertFlush;
  remoteUpsertFlush = new Promise<void>((resolve) => {
    window.setTimeout(() => {
      const batch = [...pendingRemoteUpserts.values()];
      pendingRemoteUpserts.clear();
      remoteUpsertFlush = undefined;
      void applyRemoteUpserts(batch).finally(resolve);
    }, 200);
  });
  return remoteUpsertFlush;
}

/** Browser-preview variant of a remote upsert: plain local list surgery. */
function upsertLocalEntry(entry: ClipboardEntry): void {
  previewEntries.value = [
    { ...entry, summary: EMPTY_SUMMARY },
    ...previewEntries.value.filter((item) => item.id !== entry.id),
  ].sort((a, b) => b.createdAt.localeCompare(a.createdAt));
}

async function applyRemoteUpserts(batch: ClipboardEntry[]): Promise<void> {
  for (const entry of batch) markEntrySynced(entry);
  if (!runningInTauri) {
    for (const entry of batch) upsertLocalEntry(entry);
    return;
  }
  try {
    await invoke("upsert_remote_entries", { entries: batch });
  } catch (error) {
    showToast(`写入同步记录失败：${errorMessage(error)}`, "error");
    return;
  }
  refreshHistory();
}

function rememberDevices(devices: Device[]): void {
  devicesById.value = {
    ...devicesById.value,
    ...Object.fromEntries(devices.map((device) => [device.id, device])),
  };
}

function markEntrySynced(entry: ClipboardEntry): void {
  if (syncedEntryIds.value.has(entry.id)) return;
  syncedEntryIds.value = new Set(syncedEntryIds.value).add(entry.id);
}

function isEntrySynced(entry: ClipboardEntry): boolean {
  return syncedEntryIds.value.has(entry.id);
}

function focusSearch(): void {
  void nextTick(() => historyView.value?.focusSearch());
}

// Mirrors the former Rust-side paste positioning: center the window below the
// cursor and clamp it inside the monitor's work area. All values are physical
// pixels.
function calculatePasteWindowPosition(
  cursorX: number,
  cursorY: number,
  workX: number,
  workY: number,
  workWidth: number,
  workHeight: number,
  windowWidth: number,
  windowHeight: number,
): { x: number; y: number } {
  const CURSOR_GAP = 12;
  const SCREEN_MARGIN = 8;
  const minX = workX + SCREEN_MARGIN;
  const minY = workY + SCREEN_MARGIN;
  const maxX = Math.max(workX + workWidth - windowWidth - SCREEN_MARGIN, minX);
  const maxY = Math.max(workY + workHeight - windowHeight - SCREEN_MARGIN, minY);
  const x = Math.min(Math.max(cursorX - Math.floor(windowWidth / 2), minX), maxX);
  const belowCursor = cursorY + CURSOR_GAP;
  const preferredY = belowCursor <= maxY ? belowCursor : cursorY - windowHeight - CURSOR_GAP;
  return { x, y: Math.min(Math.max(preferredY, minY), maxY) };
}

async function showPasteWindow(): Promise<void> {
  if (!isPasteWindow || !runningInTauri) return;
  const pasteWindow = getCurrentWindow();
  try {
    const cursor = await cursorPosition();
    const monitor: Monitor | null = await monitorFromPoint(cursor.x, cursor.y);
    if (monitor) {
      const windowSize = await pasteWindow.outerSize();
      const workArea = monitor.workArea;
      const position = calculatePasteWindowPosition(
        Math.round(cursor.x),
        Math.round(cursor.y),
        workArea.position.x,
        workArea.position.y,
        workArea.size.width,
        workArea.size.height,
        windowSize.width,
        windowSize.height,
      );
      await pasteWindow.setPosition(new PhysicalPosition(position.x, position.y));
    }
  } catch (error) {
    // The window still opens even if positioning is unavailable.
    console.error("定位快捷粘贴窗口失败：", error);
  }
  await pasteWindow.show();
  await pasteWindow.unminimize();
  await pasteWindow.setFocus();
  focusSearch();
}

function shareImportMessage(summary: ShareImportSummary): string {
  const parts = [
    summary.texts ? `${summary.texts} 条文字` : "",
    summary.images ? `${summary.images} 张图片` : "",
    summary.files ? `${summary.files} 个文件` : "",
  ].filter(Boolean);
  return parts.length ? `已接收${parts.join("、")}` : "";
}

async function consumeMobileShares(): Promise<void> {
  if (!runningInTauri || !platformCapabilities.value.shareReceiver || importingShare.value) return;
  importingShare.value = true;
  try {
    const summary = await invoke<ShareImportSummary>("consume_mobile_shares");
    if (!summary.shares) return;
    refreshHistory();
    showToast(shareImportMessage(summary), "success");
  } catch (error) {
    showToast(`接收系统分享失败：${errorMessage(error)}，请重新分享`, "error");
  } finally {
    importingShare.value = false;
  }
}

async function hideWindow(): Promise<void> {
  if (runningInTauri && !isMobile.value) await invoke(isPasteWindow ? "hide_paste" : "hide_main");
}

/**
 * Downloads the given contents through the fixed transfer pool, reporting
 * per-entry progress and failing with the aggregate failure count. One failure
 * does not abort the rest — a 3000-file folder should not be lost to a single
 * bad transfer.
 */
async function downloadMissingFiles(
  entryId: string,
  missing: MissingFile[],
  downloadOne: (file: MissingFile) => Promise<void>,
): Promise<void> {
  let finished = 0;
  const reportProgress = () => {
    downloadProgressByEntryId.value = {
      ...downloadProgressByEntryId.value,
      [entryId]: { finished, total: missing.length },
    };
  };
  reportProgress();
  const results = await mapWithConcurrency(missing, TRANSFER_CONCURRENCY, async (file) => {
    await downloadOne(file);
    finished += 1;
    reportProgress();
  });
  const failures = results.filter((result) => result.status === "rejected").length;
  if (failures) {
    throw new Error(`有 ${failures} 个文件下载失败（共 ${missing.length} 个）`);
  }
}

function withoutKey<T>(record: Record<string, T>, id: string): Record<string, T> {
  const { [id]: _, ...remaining } = record;
  return remaining;
}

/**
 * Fetches every content this device is missing.
 */
async function downloadRequiredFiles(
  entry: LocalClipboardEntry,
  prepareCommand: "prepare_entry_files" | "prepare_paste_entry",
): Promise<LocalClipboardEntry> {
  if (entry.kind !== "files" && entry.kind !== "image") return entry;
  const missing = await invoke<MissingFile[]>(prepareCommand, { entryId: entry.id });
  if (!missing.length) return entry;
  const client = syncClient;
  if (!client) throw new Error("同步服务未连接，无法获取其他设备的文件");

  try {
    await downloadMissingFiles(entry.id, missing, (file) =>
      client.downloadFile(entry, { fileId: file.fileId, size: file.size }));
  } finally {
    downloadProgressByEntryId.value = withoutKey(downloadProgressByEntryId.value, entry.id);
    refreshHistory();
  }
  // Re-read the persisted entry: its availability summary changed on disk.
  return (await fullEntry(entry)) as LocalClipboardEntry;
}

async function ensureLocalFiles(entry: LocalClipboardEntry): Promise<LocalClipboardEntry> {
  return downloadRequiredFiles(entry, "prepare_entry_files");
}

async function ensurePasteReady(entry: LocalClipboardEntry): Promise<LocalClipboardEntry> {
  return downloadRequiredFiles(entry, "prepare_paste_entry");
}

async function activateEntry(
  entry: LocalClipboardEntry | undefined,
  command: "copy_entry" | "paste_entry",
): Promise<void> {
  if (!entry) return;
  if (!runningInTauri) {
    await navigator.clipboard.writeText(entry.content);
    showToast("已复制到系统剪贴板", "success");
    return;
  }
  if (isMobile.value && entry.kind !== "text") {
    await saveEntry(entry);
    return;
  }
  if (activatingEntryId.value) return;
  activatingEntryId.value = entry.id;
  try {
    // Rust selects the native strategy. This downloads only what the current
    // platform must materialize before it can copy or paste the entry.
    await ensurePasteReady(entry);
    await invoke(command, { entryId: entry.id });
    showToast(command === "copy_entry" ? "已复制到系统剪贴板" : "已粘贴到当前应用", "success");
  } catch (error) {
    if (String(error).includes("clipboard entry was not found")) {
      refreshHistory();
      return;
    }
    showToast(String(error), "error");
  } finally {
    activatingEntryId.value = "";
  }
}

function copyEntry(entry?: LocalClipboardEntry): Promise<void> {
  return activateEntry(entry, "copy_entry");
}

function pasteEntry(entry?: LocalClipboardEntry): Promise<void> {
  return activateEntry(entry, "paste_entry");
}

/**
 * A larger automatic-upload limit can make old local files newly eligible.
 * Nothing is published — these entries already live on the server — so this
 * only re-uploads their contents, which the content-addressed pool absorbs.
 */
async function uploadNowEligibleEntries(sizeLimit: number): Promise<void> {
  const client = syncClient;
  if (!client || sizeLimit <= 0) return;
  // The candidate filter (kind, hashing state, size limit) runs Rust-side over
  // the durable history; the browser preview filters its demo list instead.
  const candidates = runningInTauri
    ? await invoke<LocalClipboardEntry[]>("list_upload_candidates", { limitBytes: sizeLimit }).catch(() => [])
    : previewEntries.value.filter((entry) => (
      (entry.kind === "files" || entry.kind === "image")
      && !isHashing(entry)
      && entry.summary.uploadableSize !== undefined
      && entry.summary.uploadableSize < sizeLimit
    ));
  for (const entry of candidates) {
    if (syncClient !== client) return;
    try {
      // Uploads need the directory tree, so it is fetched per entry.
      await client.uploadEntryContents(await fullEntry(entry));
    } catch (error) {
      if (syncClient === client) {
        showToast(`自动上传失败：${errorMessage(error)}`, "error");
      }
    }
  }
  if (syncClient === client) refreshHistory();
}

async function saveEntry(entry: LocalClipboardEntry): Promise<void> {
  if (savingEntryId.value || !canSaveEntry(entry)) return;
  savingEntryId.value = entry.id;
  let saveId: string | undefined;
  try {
    if (isMobile.value) {
      await ensureLocalFiles(entry);
      showToast("内容已下载到应用缓存，可在 ClipRoam 中离线使用", "success");
    } else {
      const preparation = await invoke<SavePreparation | null>("prepare_save_entry", {
        entryId: entry.id,
      });
      if (!preparation) return;
      saveId = preparation.saveId;

      if (preparation.missing.length) {
        const client = syncClient;
        if (!client) throw new Error("同步服务未连接，无法获取其他设备的文件");
        await downloadMissingFiles(entry.id, preparation.missing, (file) =>
          client.downloadFileToSave(entry, { fileId: file.fileId, size: file.size }, preparation.saveId));
      }

      const saved = await invoke<number>("finish_save_entry", { saveId: preparation.saveId });
      saveId = undefined;
      if (saved > 0) showToast(`已保存 ${saved} 个文件`, "success");
    }
  } catch (error) {
    if (saveId) await invoke("cancel_save_entry", { saveId }).catch(() => undefined);
    showToast(`${isMobile.value ? "下载" : "另存为"}失败：${errorMessage(error)}`, "error");
  } finally {
    downloadProgressByEntryId.value = withoutKey(downloadProgressByEntryId.value, entry.id);
    savingEntryId.value = "";
  }
}

/**
 * Activation requests from the history view. `viaClick` mirrors the old
 * select-or-activate split: clicks activate immediately only in the paste
 * window and on mobile, keyboard/double-click everywhere.
 */
function activateFromView(entry: LocalClipboardEntry, viaClick: boolean): void {
  if (viaClick) {
    if (isPasteWindow) void pasteEntry(entry);
    else if (isMobile.value) void copyEntry(entry);
    return;
  }
  if (isPasteWindow) void pasteEntry(entry);
  else if (entry.kind === "files") {
    showToast("文件请使用 Ctrl+Shift+V 快捷粘贴，或点击“另存为…”手动下载", "info");
  } else {
    void copyEntry(entry);
  }
}

/** A pending row's display id is `p{seq}`; extract the seq its Dequeue takes. */
function pendingSeqOf(entryId: string): number | null {
  return /^p\d+$/.test(entryId) ? Number(entryId.slice(1)) : null;
}

// Deletion is server-authoritative: the request goes out, and the local entry
// is only cleaned up when the `clipboard.deleted` echo arrives (the server
// broadcasts to every device, including the initiator). A pending queue row
// never reached the server, so it is dequeued outright.
async function removeEntry(entry: ClipboardEntry): Promise<void> {
  const seq = pendingSeqOf(entry.id);
  if (seq !== null) {
    await invoke("dequeue_pending_entry", { seq }).catch((error) => {
      showToast(`删除失败：${errorMessage(error)}`, "error");
    });
    return;
  }
  const client = syncClient;
  if (!client) {
    showToast("网络异常，暂时无法删除，请检查同步连接", "error");
    return;
  }
  try {
    await client.delete(entry.id);
  } catch (error) {
    showToast(`删除失败：${errorMessage(error)}`, "error");
    return;
  }
  // Idempotent fallback in case the echo is lost (e.g. disconnect right after
  // the response); cleanup stays a no-op if the echo already handled it.
  if (runningInTauri) {
    setTimeout(() => {
      // The command emits `cliproam://history-changed`, which refreshes the
      // views; no explicit invalidation needed here.
      void invoke("remove_remote_entry", { entryId: entry.id });
    }, 5000);
  }
}

// Invoked from the history view once its confirm dialog was accepted; the view
// owns the dialog state, the toast and the post-clear focus.
async function clearHistory(): Promise<void> {
  const client = syncClient;
  if (!client) throw new Error("网络异常，暂时无法清空，请检查同步连接");
  const entryIds = runningInTauri
    ? await invoke<string[]>("list_entry_ids")
    : previewEntries.value.map((entry) => entry.id);
  let failures = 0;
  for (const entryId of entryIds) {
    await client.delete(entryId).catch(() => { failures += 1; });
  }
  if (failures > 0) throw new Error(`${failures} 条记录删除失败，请重试`);
  // Each entry's local cleanup rides its own `clipboard.deleted` echo. Pending
  // rows never reached the server; they are dropped locally, or the drain
  // would republish them right after the clear.
  if (runningInTauri) {
    const pending = await invoke<LocalClipboardEntry[]>("list_pending_entries").catch(() => []);
    for (const entry of pending) {
      const seq = pendingSeqOf(entry.id);
      if (seq !== null) {
        await invoke("dequeue_pending_entry", { seq }).catch(() => undefined);
      }
    }
  }
}

async function getDevice(): Promise<Device> {
  const osVersion = detectOsVersion();
  if (!runningInTauri) {
    return { id: "browser", name: "浏览器预览", platform: navigator.platform || "browser", osVersion };
  }
  const [deviceId, deviceName] = await invoke<[string, string]>("get_device");
  return { id: deviceId, name: deviceName, platform: navigator.platform || "desktop", osVersion };
}

function detectOsVersion(): string {
  const userAgent = navigator.userAgent;
  const windows = userAgent.match(/Windows NT ([\d.]+)/i);
  if (windows) return `Windows NT ${windows[1]}`;
  const macOS = userAgent.match(/Mac OS X ([\d_]+)/i);
  if (macOS) return `macOS ${macOS[1].replace(/_/g, ".")}`;
  const android = userAgent.match(/Android ([\d.]+)/i);
  if (android) return `Android ${android[1]}`;
  const ios = userAgent.match(/(?:iPhone|iPad).*OS ([\d_]+)/i);
  if (ios) return `iOS ${ios[1].replace(/_/g, ".")}`;
  return navigator.platform || "未知";
}

async function loadSyncConfig(): Promise<SyncConfig | null> {
  let raw: unknown;
  if (runningInTauri) raw = await invoke<unknown>("get_sync_config");
  else {
    try {
      const stored = window.localStorage.getItem(BROWSER_CONFIG_KEY);
      raw = stored ? JSON.parse(stored) : null;
    } catch {
      return null;
    }
  }
  if (!raw || typeof raw !== "object") return null;
  const value = raw as Record<string, unknown>;
  return {
    enabled: value.enabled === true,
    serverAddress: typeof value.serverAddress === "string" && value.serverAddress
      ? value.serverAddress
      : DEFAULT_SERVER_ADDRESS,
    serverProtocol: value.serverProtocol === "https" ? "https" : DEFAULT_SERVER_PROTOCOL,
    username: typeof value.username === "string" ? value.username : "",
    sessionToken: typeof value.sessionToken === "string" ? value.sessionToken : "",
    autoUploadLimitMb: typeof value.autoUploadLimitMb === "number"
      ? Math.max(0, value.autoUploadLimitMb)
      : DEFAULT_AUTO_UPLOAD_LIMIT_MB,
    autoReceiveClipboard: value.autoReceiveClipboard !== false,
  };
}

async function persistSyncConfig(config: SyncConfig): Promise<void> {
  if (runningInTauri) await invoke("save_sync_config", { config });
  else window.localStorage.setItem(BROWSER_CONFIG_KEY, JSON.stringify(config));
}

function closeSetup(): void {
  if (testingConnection.value || !hasSavedSyncConfig.value) return;
  setupVisible.value = false;
  setupError.value = "";
  void nextTick(focusSearch);
}

async function useLocalMode(draft: SetupDraft): Promise<void> {
  if (testingConnection.value) return;
  const config: SyncConfig = {
    enabled: false,
    serverAddress: draft.serverAddress,
    serverProtocol: draft.serverProtocol,
    username: draft.username,
    sessionToken: activeSyncConfig?.sessionToken ?? "",
    autoUploadLimitMb: activeSyncConfig?.autoUploadLimitMb ?? DEFAULT_AUTO_UPLOAD_LIMIT_MB,
    autoReceiveClipboard: activeSyncConfig?.autoReceiveClipboard ?? DEFAULT_AUTO_RECEIVE_CLIPBOARD,
  };
  setupError.value = "";
  try {
    await persistSyncConfig(config);
    activeSyncConfig = config;
    currentUsername.value = config.username;
    hasSavedSyncConfig.value = true;
    stopSyncClient(false);
    setupVisible.value = false;
    refreshHistory();
    await nextTick();
    await focusSearch();
  } catch (error) {
    setupError.value = `无法保存设置：${errorMessage(error)}`;
  }
}

async function connectAndSave(draft: SetupDraft): Promise<void> {
  setupError.value = "";
  const { serverAddress, username, password, serverProtocol } = draft;

  testingConnection.value = true;
  let accountCreated = false;
  try {
    const device = await getDevice();
    const session = await authenticateAccount(
      serverAddress,
      username,
      password,
      draft.authMode,
      serverProtocol,
      device.id,
    );
    accountCreated = draft.authMode === "register";
    const { webSocketUrl } = getServerUrls(serverAddress, serverProtocol);
    await testSyncConnection(webSocketUrl, session.sessionToken, device);
    const config: SyncConfig = {
      enabled: true,
      serverAddress,
      serverProtocol,
      username: session.user.username,
      sessionToken: session.sessionToken,
      autoUploadLimitMb: DEFAULT_AUTO_UPLOAD_LIMIT_MB,
      autoReceiveClipboard: DEFAULT_AUTO_RECEIVE_CLIPBOARD,
    };
    await persistSyncConfig(config);
    activeSyncConfig = config;
    currentUsername.value = config.username;
    hasSavedSyncConfig.value = true;
    setupVisible.value = false;
    refreshHistory();
    await startSync(config);
    await nextTick();
    await focusSearch();
  } catch (error) {
    const message = errorMessage(error);
    if (accountCreated) {
      setupWizard.value?.setAuthMode("login");
      setupError.value = `账号已创建，但同步连接失败：${message}。请重新登录`;
    } else {
      setupError.value = message;
    }
  } finally {
    testingConnection.value = false;
  }
}

function handleKeys(event: KeyboardEvent): void {
  if (!isPasteWindow && settingsVisible.value) {
    if (event.key === "Escape") {
      event.preventDefault();
      closeSettings();
    }
    return;
  }
  if (setupVisible.value) {
    if (event.key === "Escape" && hasSavedSyncConfig.value) {
      event.preventDefault();
      closeSetup();
    }
    return;
  }
  // The history view handles its own dialogs (clear-history confirm, image
  // preview) plus selection keys; a true return means the key was consumed.
  if (historyView.value?.handleKeydown(event)) return;
  if (event.key === "Escape") {
    event.preventDefault();
    void hideWindow();
  }
}

async function activateRemoteClipboard(entry: ClipboardEntry): Promise<void> {
  const config = activeSyncConfig;
  if (
    !runningInTauri
    || !config?.autoReceiveClipboard
    || entry.kind === "files"
    || (isMobile.value && entry.kind !== "text")
  ) return;

  const activationRevision = ++remoteActivationRevision;
  const startingLocalRevision = localClipboardRevision;
  try {
    // The activation carries the complete entry so it remains safe even when
    // its history update and activation messages are handled concurrently —
    // once `applyRemoteUpserts` resolves it is durable, no re-read needed.
    await applyRemoteUpserts([entry]);
    let localEntry = entry as LocalClipboardEntry;
    if (activeSyncConfig !== config || !config.autoReceiveClipboard) return;
    if (entry.kind === "image") localEntry = await ensurePasteReady(localEntry);

    // A newer remote activation or a real local copy wins while an image is
    // downloading; never replace content the user copied in the meantime.
    if (
      activationRevision !== remoteActivationRevision
      || startingLocalRevision !== localClipboardRevision
      || activeSyncConfig !== config
      || !config.autoReceiveClipboard
    ) return;
    await invoke("activate_remote_entry", { entryId: localEntry.id });
  } catch (error) {
    if (
      activationRevision === remoteActivationRevision
      && activeSyncConfig === config
      && config.autoReceiveClipboard
    ) {
      showToast(`自动接收剪贴板失败：${errorMessage(error)}`, "error");
    }
  }
}

/**
 * The rendered list carries only aggregates. Publishing needs the directory tree,
 * so it is fetched per entry instead of for the whole history.
 */
async function fullEntry(entry: Pick<ClipboardEntry, "id">): Promise<ClipboardEntry> {
  if (!runningInTauri) return entry as ClipboardEntry;
  return invoke<ClipboardEntry>("get_entry", { entryId: entry.id });
}

async function reconcileManifest(manifest: ClipboardManifestEntry[]): Promise<void> {
  const client = syncClient;
  if (!client) return;

  try {
    // The manifest covers only the newest page of server rows, so marks merge
    // instead of rebuilding: entries older than that page keep the "synced"
    // state they were given when last seen in a manifest. Remote deletions
    // still clear marks through the `clipboard.deleted` push.
    const knownSynced = new Set(syncedEntryIds.value);
    for (const entry of manifest) {
      knownSynced.add(entry.id);
    }
    syncedEntryIds.value = knownSynced;
    // Read the durable history rather than the rendered list. The latter can
    // be stale while another Tauri window is refreshing it; ids alone suffice
    // for the diff, so no whole-history read is needed.
    const localEntryIds = runningInTauri
      ? await invoke<string[]>("list_entry_ids")
      : previewEntries.value.map((entry) => entry.id);
    const localClientIds = new Set(localEntryIds);
    const remoteOnlyEntryIds = manifest
      .filter((entry) => !localClientIds.has(entry.id))
      .map((entry) => entry.id);
    const remoteEntries = await client.fetchEntries(remoteOnlyEntryIds);

    // One batch write for the whole gap instead of a full history rewrite and
    // list refresh per entry.
    if (remoteEntries.length && syncClient === client) {
      await applyRemoteUpserts(remoteEntries);
    }

    refreshHistory();
    // A reconcile is also the moment the live availability view is re-derived
    // against the pool before the drain republishes the capture queue.
    if (syncClient === client) await refreshStoredFileIds();
    // Deletions and remote upserts have been replayed; now publish whatever
    // the durable capture queue still holds (single-flight, no-op if running).
    if (syncClient === client) client.drainQueue();
  } catch (error) {
    if (syncClient === client) {
      showToast(`同步历史失败：${errorMessage(error)}`, "error");
    }
  }
}

async function startSync(config: SyncConfig): Promise<void> {
  // The quick-paste window only reads local history; broadcasts from the main
  // window keep it fresh, and a second socket would double every sync task.
  if (isPasteWindow) return;
  syncClient?.stop();
  connected.value = false;
  syncEnabled.value = true;
  const device = await getDevice();
  const { httpUrl, webSocketUrl } = getServerUrls(config.serverAddress, config.serverProtocol);
  let client: SyncClient;
  client = new SyncClient(
    httpUrl,
    webSocketUrl,
    config.sessionToken,
    device,
    {
      onConnected: (value) => {
        connected.value = value;
        // Offline captures replay the moment the socket comes back.
        syncClient?.drainQueue();
      },
      onManifest: (manifest, devices) => {
        rememberDevices(devices);
        void reconcileManifest(manifest);
      },
      onDevicePresence: (device) => { rememberDevices([device]); },
      onEntry: (entry) => {
        void queueRemoteUpsert(entry);
      },
      onActivation: (entry) => {
        if (syncClient === client) void activateRemoteClipboard(entry);
      },
      onDelete: (entryId) => {
        const remaining = new Set(syncedEntryIds.value);
        remaining.delete(entryId);
        syncedEntryIds.value = remaining;
        if (runningInTauri) void invoke("remove_remote_entry", { entryId });
        else previewEntries.value = previewEntries.value.filter((entry) => entry.id !== entryId);
      },
      onFileAvailable: (fileId) => {
        // Content-addressed push: the server now holds this content. Kept
        // in-memory only — nothing survives a restart; the next pool query
        // re-derives it.
        const next = new Set(storedFileIds.value);
        next.add(fileId);
        storedFileIds.value = next;
        refreshHistory();
      },
      onUploadProgress: (entryId, uploadedBytes, totalBytes) => {
        uploadProgressByEntryId.value = {
          ...uploadProgressByEntryId.value,
          [entryId]: { uploadedBytes, totalBytes },
        };
      },
      onUploadFinished: (entryId) => {
        uploadProgressByEntryId.value = withoutKey(uploadProgressByEntryId.value, entryId);
      },
      onError: (message) => { showToast(message, "error"); },
      onAuthenticationFailed: (message) => {
        if (syncClient !== client) return;
        stopSyncClient();
        const alreadyRelogging = setupVisible.value && !activeSyncConfig?.sessionToken;
        const expiredConfig = { ...config, sessionToken: "" };
        activeSyncConfig = expiredConfig;
        currentUsername.value = expiredConfig.username;
        setupError.value = message;
        setupVisible.value = true;
        if (!alreadyRelogging) {
          void persistSyncConfig(expiredConfig);
          void nextTick(() => setupWizard.value?.setFields(expiredConfig));
        }
      },
    },
    config.autoUploadLimitMb * 1024 * 1024,
  );
  syncClient = client;
  client.connect();
}

function withStartupTimeout<T>(promise: Promise<T>, message: string): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = window.setTimeout(() => reject(new Error(message)), 5_000);
    promise.then(
      (value) => {
        window.clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        window.clearTimeout(timer);
        reject(error);
      },
    );
  });
}

function readableStartupError(error: unknown): string {
  const message = errorMessage(error);
  if (message.includes("state not managed for field `state`")) {
    return "应用初始化尚未完成，请从托盘退出 ClipRoam 后重新启动";
  }
  return message || "未知错误";
}

async function initializeTauriServices(): Promise<void> {
  const listenerResults = await Promise.allSettled([
    startToastWindowListener(),
    listen("cliproam://entry-created", () => {
      localClipboardRevision += 1;
      refreshHistory();
      // Hashing (for files) and publishing both happen inside the drain, which
      // Rust restarts after each row it resolves.
      syncClient?.drainQueue();
    }),
    listen("cliproam://history-changed", refreshHistory),
    listen("cliproam://show-paste", () => { void showPasteWindow(); }),
    listen<VirtualFileRequest>("cliproam://virtual-file-request", async ({ payload }) => {
      const client = syncClient;
      if (!client) {
        await invoke("fail_virtual_file_request", {
          fileId: payload.fileId,
          message: "同步服务未连接，无法获取其他设备的文件",
        });
        return;
      }
      try {
        await client.downloadVirtualFile(payload);
        refreshHistory();
      } catch (error) {
        await invoke("fail_virtual_file_request", {
          fileId: payload.fileId,
          message: errorMessage(error),
        }).catch(() => undefined);
      }
    }),
  ]);
  unlisteners = listenerResults.flatMap((result) => result.status === "fulfilled" ? [result.value] : []);
  const listenerError = listenerResults.find((result) => result.status === "rejected");
  if (listenerError?.status === "rejected") {
    showToast(`部分后台事件监听初始化失败：${String(listenerError.reason)}`, "error");
  }

  if (!isPasteWindow && platformCapabilities.value.globalShortcut) {
    const registered = await initializeQuickPasteShortcut();
    if (!registered) showToast(quickPasteShortcutStatus.value.message, "error");
  }

  if (!isPasteWindow && platformCapabilities.value.shareReceiver) {
    try {
      shareReceiverListener = await addPluginListener<ShareReceiverEvent>(
        "cliproam-share-receiver",
        "received",
        (payload) => {
          if (payload.error) {
            showToast(`接收系统分享失败：${payload.error}，请重新分享`, "error");
          } else {
            void consumeMobileShares();
          }
        },
      );
      await consumeMobileShares();
    } catch (error) {
      showToast(`系统分享接收初始化失败：${errorMessage(error)}`, "error");
    }
  }
}

onMounted(async () => {
  if (isToastWindow) {
    unlisteners = [await startToastWindowListener()];
    return;
  }

  ageRefreshTimer = window.setInterval(() => { currentTime.value = Date.now(); }, 10_000);
  document.addEventListener("keydown", handleKeys);

  let config: SyncConfig | null = null;
  let startupWarning = "";
  const platformPromise = runningInTauri
    ? withStartupTimeout(
        invoke<PlatformCapabilities>("get_platform_capabilities"),
        "读取平台能力超时",
      )
    : Promise.resolve(DESKTOP_CAPABILITIES);
  const [capabilitiesResult, configResult] = await Promise.allSettled([
    platformPromise,
    withStartupTimeout(loadSyncConfig(), "读取连接配置超时"),
  ]);
  if (capabilitiesResult.status === "fulfilled") {
    setPlatformCapabilities(capabilitiesResult.value);
  } else {
    startupWarning = `平台能力读取失败：${readableStartupError(capabilitiesResult.reason)}`;
  }
  if (configResult.status === "fulfilled") {
    config = configResult.value;
  } else {
    setupError.value = `无法读取连接设置：${readableStartupError(configResult.reason)}`;
    startupWarning = setupError.value;
  }

  if (!config) {
    if (!isPasteWindow) setupVisible.value = true;
  } else {
    activeSyncConfig = config;
    syncEnabled.value = config.enabled;
    currentUsername.value = config.username;
    hasSavedSyncConfig.value = true;
    if (config.enabled && (!config.username || !config.sessionToken)) setupVisible.value = true;
  }
  initializing.value = false;

  if (startupWarning) showToast(startupWarning, "error");
  if (!isPasteWindow) void initUpdaterVersion();
  // The history view fetches its first page itself (revision watch with
  // `immediate`); only the pending badge needs an initial query.
  void refreshPendingCount();
  if (runningInTauri) void initializeTauriServices();

  if (setupVisible.value) {
    await nextTick();
    setupWizard.value?.setFields(config ?? undefined);
    setupWizard.value?.focusServerInput();
  } else if (config?.enabled && config.username && config.sessionToken) {
    try {
      await startSync(config);
    } catch (error) {
      showToast(`同步初始化失败：${errorMessage(error)}`, "error");
    }
    await focusSearch();
  } else if (config) {
    await focusSearch();
  }
});

onBeforeUnmount(() => {
  if (ageRefreshTimer !== undefined) window.clearInterval(ageRefreshTimer);
  disposeToast();
  if (refreshTimer !== undefined) window.clearTimeout(refreshTimer);
  if (pendingRemoteUpserts.size) {
    void applyRemoteUpserts([...pendingRemoteUpserts.values()]);
    pendingRemoteUpserts.clear();
  }
  document.removeEventListener("keydown", handleKeys);
  unlisteners.forEach((unlisten) => unlisten());
  if (shareReceiverListener) void shareReceiverListener.unregister();
  syncClient?.stop();
  if (!isPasteWindow) void disposeQuickPasteShortcut();
});
</script>

<template>
  <main v-if="isToastWindow" class="toast-window-shell" aria-hidden="true"></main>

  <main v-else-if="initializing" class="setup-shell setup-loading" :class="{ 'mobile-shell': isMobile }">
    <section class="setup-loading-content" role="status" aria-live="polite">
      <span class="setup-icon" aria-hidden="true"><LoaderCircle :size="24" class="spin" /></span>
      <strong>ClipRoam</strong>
      <span>正在读取连接配置…</span>
    </section>
  </main>

  <main v-else-if="setupVisible" class="setup-shell" :class="{ 'mobile-shell': isMobile }">
    <SetupWizard
      ref="setupWizard"
      :has-saved-sync-config="hasSavedSyncConfig"
      :busy="testingConnection"
      :error="setupError"
      @submit="connectAndSave"
      @local="useLocalMode"
      @close="closeSetup"
      @reset-error="setupError = ''"
    />
  </main>

  <main v-else class="app-shell" :class="{ 'paste-app': isPasteWindow, 'mobile-app': isMobile }">
    <aside v-if="!isPasteWindow && !isMobile" class="sidebar" aria-label="主导航">
      <header class="sidebar-brand">
        <span class="brand-mark"><Clipboard :size="17" /></span>
        <span>
          <strong>ClipRoam</strong>
          <small>剪贴板工作区</small>
        </span>
      </header>

      <nav class="sidebar-nav" aria-label="功能模块">
        <span class="nav-section-label">工作区</span>
        <button
          class="nav-item"
          :class="{ active: activeView === 'history' }"
          type="button"
          :aria-current="activeView === 'history' ? 'page' : undefined"
          @click="activeView = 'history'"
        >
          <Clipboard :size="17" aria-hidden="true" />
          <span>剪贴板历史</span>
        </button>
        <button
          class="nav-item"
          :class="{ active: activeView === 'pending-sync' }"
          type="button"
          :aria-current="activeView === 'pending-sync' ? 'page' : undefined"
          @click="activeView = 'pending-sync'"
        >
          <CloudUpload :size="17" aria-hidden="true" />
          <span>待同步</span>
          <span v-if="pendingCount" class="nav-count">{{ pendingCount }}</span>
        </button>
      </nav>

      <div class="sidebar-bottom">
        <button class="nav-item" :class="{ active: settingsVisible }" type="button" @click="openSettings">
          <Settings2 :size="17" aria-hidden="true" />
          <span>设置</span>
        </button>
        <div class="sidebar-status" :class="connectionStatus.tone" :title="connectionStatus.title">
          <Cloud v-if="connected" :size="15" aria-hidden="true" />
          <CloudOff v-else :size="15" aria-hidden="true" />
          <span>{{ connectionStatus.label }}</span>
        </div>
      </div>
    </aside>

    <HistoryView
      v-if="activeView === 'history'"
      ref="historyView"
      :fetch-manifest="fetchManifest"
      :revision="historyRevision"
      :total-entries="totalEntryCount"
      :devices-by-id="devicesById"
      :synced-entry-ids="syncedEntryIds"
      :connection-status="connectionStatus"
      :current-time="currentTime"
      :importing-share="importingShare"
      :activating-entry-id="activatingEntryId"
      :saving-entry-id="savingEntryId"
      :upload-progress-by-entry-id="uploadProgressByEntryId"
      :download-progress-by-entry-id="downloadProgressByEntryId"
      :stored-file-ids="storedFileIds"
      :ensure-local-files="ensureLocalFiles"
      :clear-history="clearHistory"
      @activate="activateFromView"
      @remove="removeEntry"
      @save="saveEntry"
      @refresh="refreshHistory"
      @open-settings="openSettings"
    />

    <PendingSyncView
      v-else
      :entries="pendingEntries"
      :devices-by-id="devicesById"
      :current-time="currentTime"
      @remove="removeEntry"
      @back="activeView = 'history'"
    />

    <SettingsDialog
      v-if="!isPasteWindow && settingsVisible"
      :current-username="currentUsername"
      :sync-enabled="syncEnabled"
    />

  </main>

  <UpdaterDialog v-if="!isPasteWindow && !isToastWindow" locale="zh-CN" />

  <ToastLayer />
</template>
