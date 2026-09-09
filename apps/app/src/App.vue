<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { addPluginListener, invoke, type PluginListener } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { cursorPosition, getCurrentWindow, monitorFromPoint, PhysicalPosition, type Monitor } from "@tauri-apps/api/window";
import { UpdaterDialog } from "@while-coder/tauri-updater-vue";
import type {
  ClipboardEntry,
  ClipboardManifestEntry,
} from "@cliproam/protocol";
import { entryContents, DEFAULT_AUTO_RECEIVE_CLIPBOARD, DEFAULT_AUTO_UPLOAD_LIMIT_MB, DEFAULT_SERVER_PROTOCOL } from "@cliproam/protocol";
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
  type ServerProtocol,
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
} from "./utils/constants";
import { canManualUpload, canSaveEntry, isHashing } from "./utils/entry";
import { errorMessage } from "./utils/error";
import { isToastWindow, isPasteWindow, runningInTauri, usePlatform } from "./composables/usePlatform";
import type {
  Device,
  DownloadProgress,
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

const entries = ref<LocalClipboardEntry[]>([]);
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
const uploadingEntryId = ref("");
const uploadProgressByEntryId = ref<Record<string, UploadProgress>>({});
const downloadProgressByEntryId = ref<Record<string, DownloadProgress>>({});
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
 * Entries the server manifest does not know about: captured offline, queued for
 * publishing, or waiting for their content ids (hashing). Text syncs on its
 * own; files and images additionally need a content upload.
 */
const pendingEntries = computed(() => (
  entries.value.filter((entry) => !isEntrySynced(entry))
));

async function refreshEntries(): Promise<void> {
  if (!runningInTauri) {
    entries.value = demoEntries;
    return;
  }
  entries.value = await invoke<LocalClipboardEntry[]>("list_entries");
}

let refreshEntriesTimer: number | undefined;

/**
 * Background events (hashing, remote upserts, file availability) arrive in
 * bursts. Each `list_entries` round-trip re-serializes the whole history, so a
 * burst coalesces into one refresh instead of one per event.
 */
function scheduleRefreshEntries(): void {
  if (refreshEntriesTimer !== undefined) return;
  refreshEntriesTimer = window.setTimeout(() => {
    refreshEntriesTimer = undefined;
    void refreshEntries().catch((error) => {
      showToast(`剪贴板历史读取失败：${errorMessage(error)}`, "error");
    });
  }, 200);
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

/**
 * Which of a batch's contents the server's pool holds. This replaces the
 * per-entry `missing` list the protocol dropped: one query per upsert batch,
 * and a failure degrades to "nothing is uploaded" — re-beginning an upload of
 * content the server actually has costs one cheap `stored` answer. Contents
 * this device already caches or knows are stored are filtered out Rust-side,
 * so they never ride the request.
 */
async function serverAvailableFileIds(batch: ClipboardEntry[]): Promise<string[]> {
  const client = syncClient;
  if (!client) return [];
  const fileIds = [...new Set(batch.flatMap((entry) => entryContents(entry).map(({ fileId }) => fileId)))];
  if (!fileIds.length) return [];
  try {
    const unknown = runningInTauri
      ? await invoke<string[]>("filter_unknown_file_ids", { fileIds })
      : fileIds;
    if (!unknown.length) return [];
    const statuses = await client.fetchFileStatuses(unknown);
    return statuses.filter((file) => file.stored).map((file) => file.fileId);
  } catch (error) {
    showToast(`查询服务器文件状态失败：${errorMessage(error)}`, "error");
    return [];
  }
}

/** Browser-preview variant of a remote upsert: plain local list surgery. */
function upsertLocalEntry(entry: ClipboardEntry): void {
  entries.value = [
    { ...entry, summary: EMPTY_SUMMARY },
    ...entries.value.filter((item) => item.id !== entry.id),
  ].sort((a, b) => b.createdAt.localeCompare(a.createdAt));
}

async function applyRemoteUpserts(batch: ClipboardEntry[]): Promise<void> {
  for (const entry of batch) markEntrySynced(entry);
  if (!runningInTauri) {
    for (const entry of batch) upsertLocalEntry(entry);
    return;
  }
  try {
    const availableFileIds = await serverAvailableFileIds(batch);
    await invoke("upsert_remote_entries", { entries: batch, availableFileIds });
  } catch (error) {
    showToast(`写入同步记录失败：${errorMessage(error)}`, "error");
    return;
  }
  scheduleRefreshEntries();
}

/**
 * A publish response carries the server's identity: its id and timestamp
 * replace the local content-hash id so every later operation keys on the
 * server's space. Returns false only when the local record could not be
 * updated to the server's key — callers keeping an upload queue must keep
 * the entry queued in that case. An entry deleted locally while the publish
 * was in flight still adopts fine; the just-created server row follows it.
 */
async function adoptPublishedEntry(localEntryId: string, entry: ClipboardEntry): Promise<boolean> {
  if (!runningInTauri) return true;
  try {
    const adopted = await invoke<boolean>("apply_published_entry", { localEntryId, entry });
    if (!adopted) await syncClient?.delete(entry.id).catch(() => undefined);
    return true;
  } catch (error) {
    // Local and server keys can only re-converge through the next reconcile.
    showToast(`同步本地记录失败：${errorMessage(error)}`, "error");
    return false;
  }
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
    await refreshEntries();
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
    await invoke("refresh_entry", { entryId: entry.id }).catch(() => undefined);
    await refreshEntries();
  }
  return entries.value.find((candidate) => candidate.id === entry.id) ?? entry;
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
      await refreshEntries();
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

async function uploadEntry(entry: LocalClipboardEntry): Promise<void> {
  if (!syncClient) {
    showToast("同步服务未连接，无法上传文件", "error");
    return;
  }
  if (uploadingEntryId.value || uploadProgressByEntryId.value[entry.id] || !canManualUpload(entry)) return;
  uploadingEntryId.value = entry.id;
  try {
    // Publishing needs the tree, which the rendered list does not carry.
    const stored = await syncClient.upload(await fullEntry(entry));
    await adoptPublishedEntry(entry.id, stored);
  } catch (error) {
    showToast(`上传失败：${errorMessage(error)}`, "error");
  } finally {
    await refreshEntries();
    uploadingEntryId.value = "";
  }
}

/**
 * A larger automatic-upload limit can make old local files newly eligible.
 * Reuse the normal publish path so entries that already exist on the server
 * only transfer their missing contents and keep the usual progress feedback.
 */
async function uploadNowEligibleEntries(sizeLimit: number): Promise<void> {
  const client = syncClient;
  if (!client || sizeLimit <= 0) return;
  const candidates = entries.value.filter((entry) => (
    (entry.kind === "files" || entry.kind === "image")
    && !isHashing(entry)
    && entry.summary.uploadableSize !== undefined
    && entry.summary.uploadableSize < sizeLimit
  ));
  for (const entry of candidates) {
    if (syncClient !== client) return;
    try {
      const stored = await client.publish(await fullEntry(entry));
      await adoptPublishedEntry(entry.id, stored);
    } catch (error) {
      if (syncClient === client) {
        showToast(`自动上传失败：${errorMessage(error)}`, "error");
      }
    }
  }
  if (syncClient === client) await refreshEntries();
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

async function removeEntry(entry: ClipboardEntry): Promise<void> {
  if (runningInTauri) await invoke("delete_entry", { entryId: entry.id });
  else entries.value = entries.value.filter((item) => item.id !== entry.id);
  void syncClient?.delete(entry.id).catch(() => undefined);
  await refreshEntries();
}

// Invoked from the history view once its confirm dialog was accepted; the view
// owns the dialog state, the toast and the post-clear focus.
async function clearHistory(): Promise<void> {
  if (runningInTauri) await invoke("clear_history");
  else entries.value = [];
  await refreshEntries();
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
  let serverAddress = typeof value.serverAddress === "string" ? value.serverAddress : "";
  let serverProtocol: ServerProtocol = value.serverProtocol === "https" ? "https" : DEFAULT_SERVER_PROTOCOL;
  if (serverAddress.includes("://")) {
    try {
      const url = new URL(serverAddress);
      serverAddress = url.host;
      if (value.serverProtocol === undefined && url.protocol === "https:") serverProtocol = "https";
    } catch { serverAddress = ""; }
  }
  return {
    enabled: value.enabled === true,
    serverAddress: serverAddress || DEFAULT_SERVER_ADDRESS,
    serverProtocol,
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
    await refreshEntries();
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
    await refreshEntries();
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

// Only reachable through `activateRemoteClipboard`, which already returns
// outside Tauri — the browser-preview branch lives in `applyRemoteUpserts`.
async function upsertRemote(entry: ClipboardEntry): Promise<void> {
  markEntrySynced(entry);
  const availableFileIds = await serverAvailableFileIds([entry]);
  await invoke("upsert_remote_entry", { entry, availableFileIds });
  await refreshEntries();
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
    // its history update and activation messages are handled concurrently.
    await upsertRemote(entry);
    let localEntry = entries.value.find((candidate) => candidate.id === entry.id);
    if (!localEntry) throw new Error("剪贴板记录不存在");
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

/**
 * Upload marks are local bookkeeping, so they can go stale: a `file.available`
 * push missed while offline, or a failed pool query when a remote entry was
 * upserted, leaves contents marked unuploaded that the server has long since
 * stored. Each reconcile re-asks the pool about what this history still
 * believes is missing; the Rust side refreshes the affected summaries.
 */
async function refreshPendingUploadStatuses(): Promise<void> {
  const client = syncClient;
  if (!client || !runningInTauri) return;
  const pending = entries.value.filter((entry) => (
    entry.summary.contentCount > entry.summary.uploadedCount
  ));
  if (!pending.length) return;
  const pendingFileIds = [
    ...new Set(pending.flatMap((entry) => entryContents(entry).map(({ fileId }) => fileId))),
  ];
  try {
    // Contents already cached or known stored are skipped Rust-side; what
    // remains is all the pool query can still correct.
    const unknown = await invoke<string[]>("filter_unknown_file_ids", { fileIds: pendingFileIds });
    if (!unknown.length) return;
    const statuses = await client.fetchFileStatuses(unknown);
    const stored = new Set(statuses.filter((file) => file.stored).map((file) => file.fileId));
    if (!stored.size) return;
    for (const entry of pending) {
      const fileIds = entryContents(entry)
        .map(({ fileId }) => fileId)
        .filter((fileId) => stored.has(fileId));
      if (fileIds.length) {
        await invoke("mark_files_uploaded", { entryId: entry.id, fileIds });
      }
    }
  } catch (error) {
    showToast(`刷新上传状态失败：${errorMessage(error)}`, "error");
  }
}

async function reconcileManifest(manifest: ClipboardManifestEntry[]): Promise<void> {
  const client = syncClient;
  if (!client) return;

  try {
    const pendingDeletions = runningInTauri
      ? new Set(await invoke<string[]>("list_pending_deletions"))
      : new Set<string>();
    // Best effort: a failed delete keeps the id in the pending list and the
    // next reconcile retries it.
    for (const entryId of pendingDeletions) await client.delete(entryId).catch(() => undefined);
    // The manifest covers only the newest page of server rows, so marks merge
    // instead of rebuilding: entries older than that page keep the "synced"
    // state they were given when last seen in a manifest. Remote deletions
    // still clear marks through the `clipboard.deleted` push.
    const knownSynced = new Set(syncedEntryIds.value);
    for (const entry of manifest) {
      if (!pendingDeletions.has(entry.id)) knownSynced.add(entry.id);
    }
    syncedEntryIds.value = knownSynced;
    // Read the durable history rather than the rendered list. The latter can
    // be stale while another Tauri window is refreshing it.
    const localEntries = runningInTauri
      ? await invoke<LocalClipboardEntry[]>("list_entries")
      : [...entries.value];
    const localClientIds = new Set(localEntries.map((entry) => entry.id));
    const remoteOnlyEntryIds = manifest
      .filter((entry) => !localClientIds.has(entry.id) && !pendingDeletions.has(entry.id))
      .map((entry) => entry.id);
    const remoteEntries = await client.fetchEntries(remoteOnlyEntryIds);

    // One batch write for the whole gap instead of a full history rewrite and
    // list refresh per entry.
    if (remoteEntries.length && syncClient === client) {
      await applyRemoteUpserts(remoteEntries);
    }

    await refreshEntries();
    // A reconcile is also the moment stale "unuploaded" marks get corrected
    // against the pool; the drain afterwards can then skip re-uploading.
    if (syncClient === client) await refreshPendingUploadStatuses();
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
        if (runningInTauri) {
          void Promise.all([
            invoke("acknowledge_entry_deletion", { entryId }),
            invoke("remove_remote_entry", { entryId }),
          ]).then(scheduleRefreshEntries);
        }
        else entries.value = entries.value.filter((entry) => entry.id !== entryId);
      },
      onFileAvailable: (fileId) => {
        if (runningInTauri) void invoke("mark_file_available", { fileId });
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

async function applySavedSyncConfig(): Promise<void> {
  const config = await loadSyncConfig();
  if (!config) return;
  activeSyncConfig = config;
  syncEnabled.value = config.enabled;
  currentUsername.value = config.username;
  if (config.enabled && config.username && config.sessionToken) {
    await startSync(config);
  } else {
    stopSyncClient();
  }
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
      scheduleRefreshEntries();
      // The queue row already carries the full payload; hashing (for files) and
      // publishing both happen inside the drain.
      syncClient?.drainQueue();
    }),
    // Emitted once every content of an entry has a known id, which for a
    // folder happens after background hashing finishes. The drain treats the
    // row as ready only then, so this restarts any pass blocked on it.
    listen<string>("cliproam://entry-ready", async () => {
      scheduleRefreshEntries();
      if (isPasteWindow) return;
      syncClient?.drainQueue();
    }),
    listen("cliproam://history-changed", scheduleRefreshEntries),
    listen("cliproam://show-paste", () => { void showPasteWindow(); }),
    listen("cliproam://sync-config-changed", () => { void applySavedSyncConfig(); }),
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
        await invoke("refresh_entry", { entryId: payload.entryId }).catch(() => undefined);
        scheduleRefreshEntries();
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
  void refreshEntries().catch((error) => {
    showToast(`剪贴板历史读取失败：${errorMessage(error)}`, "error");
  });
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
  if (refreshEntriesTimer !== undefined) window.clearTimeout(refreshEntriesTimer);
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
          <span v-if="pendingEntries.length" class="nav-count">{{ pendingEntries.length }}</span>
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
      :entries="entries"
      :devices-by-id="devicesById"
      :synced-entry-ids="syncedEntryIds"
      :connection-status="connectionStatus"
      :current-time="currentTime"
      :importing-share="importingShare"
      :activating-entry-id="activatingEntryId"
      :uploading-entry-id="uploadingEntryId"
      :saving-entry-id="savingEntryId"
      :upload-progress-by-entry-id="uploadProgressByEntryId"
      :download-progress-by-entry-id="downloadProgressByEntryId"
      :ensure-local-files="ensureLocalFiles"
      :clear-history="clearHistory"
      @activate="activateFromView"
      @remove="removeEntry"
      @save="saveEntry"
      @upload="uploadEntry"
      @refresh="refreshEntries"
      @open-settings="openSettings"
    />

    <PendingSyncView
      v-else
      :entries="pendingEntries"
      :devices-by-id="devicesById"
      :current-time="currentTime"
      :uploading-entry-id="uploadingEntryId"
      @upload="uploadEntry"
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
