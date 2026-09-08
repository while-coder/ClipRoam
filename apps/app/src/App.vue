<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { addPluginListener, invoke, type PluginListener } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { UpdaterDialog } from "@while-coder/tauri-updater-vue";
import type {
  ClipboardEntry,
  ClipboardManifestEntry,
} from "@cliproam/protocol";
import { entryContents } from "@cliproam/protocol";
import {
  ArrowLeft,
  Check,
  Clipboard,
  Cloud,
  CloudOff,
  CloudUpload,
  FolderOpen,
  KeyRound,
  LoaderCircle,
  RefreshCw,
  Server,
  Settings2,
  ShieldCheck,
  X,
} from "lucide-vue-next";
import {
  SyncClient,
  authenticateAccount,
  changeAccountPassword,
  getServerUrls,
  normalizeServerAddress,
  testSyncConnection,
  type AuthMode,
  type ServerProtocol,
} from "./features/sync/syncClient";
import { mapWithConcurrency, TRANSFER_CONCURRENCY } from "./features/sync/concurrency";
import {
  displayShortcut,
  disposeQuickPasteShortcut,
  initializeQuickPasteShortcut,
  quickPasteShortcut,
  quickPasteShortcutRefreshing,
  quickPasteShortcutStatus,
  resetQuickPasteShortcutDraft,
  saveQuickPasteShortcut,
} from "./features/quick-paste/quickPasteShortcut";
import { useUpdater } from "./useUpdater";
import HistoryView from "./features/clipboard-history/HistoryView.vue";
import PendingSyncView from "./features/pending-sync/PendingSyncView.vue";
import ToastLayer from "./features/toast/ToastLayer.vue";
import { disposeToast, showToast, startToastWindowListener } from "./features/toast/useToast";
import {
  BROWSER_CONFIG_KEY,
  CONFIGURED_SERVER_PROTOCOL,
  DEFAULT_SERVER_ADDRESS,
  DESKTOP_CAPABILITIES,
  EMPTY_SUMMARY,
} from "./utils/constants";
import { canManualUpload, canSaveEntry, isHashing } from "./utils/entry";
import { isToastWindow, isPasteWindow, runningInTauri, usePlatform } from "./composables/usePlatform";
import type {
  Device,
  DownloadProgress,
  LocalClipboardEntry,
  MissingFile,
  PlatformCapabilities,
  SavePreparation,
  SettingsPage,
  ShareImportSummary,
  ShareReceiverEvent,
  SyncConfig,
  UploadProgress,
  VirtualFileRequest,
} from "./types";

const { platformCapabilities, isMobile, setPlatformCapabilities } = usePlatform();

const {
  appVersion,
  updaterSupported,
  updateStatus,
  updateStatusText,
  checkForUpdate,
  initUpdaterVersion,
} = useUpdater();

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
const settingsVisible = ref(false);
const settingsPage = ref<SettingsPage>("general");
const hasSavedSyncConfig = ref(false);
const setupServerAddress = ref(DEFAULT_SERVER_ADDRESS);
const setupServerProtocol = ref<ServerProtocol>(CONFIGURED_SERVER_PROTOCOL);
const setupUsername = ref("");
const setupPassword = ref("");
const authMode = ref<AuthMode>("login");
const serverFieldError = ref("");
const usernameFieldError = ref("");
const passwordFieldError = ref("");
const setupError = ref("");
const testingConnection = ref(false);
const autoUploadLimitMb = ref(10);
const autoReceiveClipboard = ref(true);
const savingSettings = ref(false);
const recordingQuickPasteShortcut = ref(false);
const changingPassword = ref(false);
const settingsError = ref("");
const passwordChangeError = ref("");
const currentPassword = ref("");
const newPassword = ref("");
const confirmNewPassword = ref("");
const currentUsername = ref("");
const importingShare = ref(false);
const activatingEntryId = ref("");
const uploadingEntryId = ref("");
const uploadProgressByEntryId = ref<Record<string, UploadProgress>>({});
const downloadProgressByEntryId = ref<Record<string, DownloadProgress>>({});
const savingEntryId = ref("");
const serverInput = ref<HTMLInputElement>();
const accountPasswordInput = ref<HTMLInputElement>();
const historyView = ref<InstanceType<typeof HistoryView>>();
let activeSyncConfig: SyncConfig | undefined;
let syncClient: SyncClient | undefined;
let unlisteners: UnlistenFn[] = [];
let ageRefreshTimer: number | undefined;
let shareReceiverListener: PluginListener | undefined;
let localClipboardRevision = 0;
let remoteActivationRevision = 0;

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
      showToast(`剪贴板历史读取失败：${error instanceof Error ? error.message : String(error)}`, "error");
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
    showToast(`查询服务器文件状态失败：${error instanceof Error ? error.message : String(error)}`, "error");
    return [];
  }
}

async function applyRemoteUpserts(batch: ClipboardEntry[]): Promise<void> {
  for (const entry of batch) markEntrySynced(entry);
  if (!runningInTauri) {
    for (const entry of batch) {
      entries.value = [
        { ...entry, summary: EMPTY_SUMMARY },
        ...entries.value.filter((item) => item.id !== entry.id),
      ].sort((a, b) => b.createdAt.localeCompare(a.createdAt));
    }
    return;
  }
  try {
    const availableFileIds = await serverAvailableFileIds(batch);
    await invoke("upsert_remote_entries", { entries: batch, availableFileIds });
  } catch (error) {
    showToast(`写入同步记录失败：${error instanceof Error ? error.message : String(error)}`, "error");
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
    showToast(`同步本地记录失败：${error instanceof Error ? error.message : String(error)}`, "error");
    return false;
  }
}

function rememberDevices(devices: Device[]): void {  devicesById.value = {
    ...devicesById.value,
    ...Object.fromEntries(devices.map((device) => [device.id, device])),
  };
}

function entrySyncId(entry: ClipboardEntry): string {
  return entry.id;
}

function markEntrySynced(entry: ClipboardEntry): void {
  const id = entrySyncId(entry);
  if (syncedEntryIds.value.has(id)) return;
  syncedEntryIds.value = new Set(syncedEntryIds.value).add(id);
}

function isEntrySynced(entry: ClipboardEntry): boolean {
  return syncedEntryIds.value.has(entrySyncId(entry));
}

function focusSearch(): void {
  void nextTick(() => historyView.value?.focusSearch());
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
    showToast(`接收系统分享失败：${error instanceof Error ? error.message : String(error)}，请重新分享`, "error");
  } finally {
    importingShare.value = false;
  }
}

async function hideWindow(): Promise<void> {
  if (runningInTauri && !isMobile.value) await invoke(isPasteWindow ? "hide_paste" : "hide_main");
}

/**
 * Fetches every content this device is missing. Downloads run through a fixed
 * pool, and one failure no longer aborts the rest — a 3000-file folder should
 * not be lost to a single bad transfer.
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

  let finished = 0;
  const reportProgress = () => {
    downloadProgressByEntryId.value = {
      ...downloadProgressByEntryId.value,
      [entry.id]: { finished, total: missing.length },
    };
  };
  reportProgress();
  try {
    const results = await mapWithConcurrency(missing, TRANSFER_CONCURRENCY, async (file) => {
      await client.downloadFile(entry, {
        fileId: file.fileId,
        size: file.size,
      });
      finished += 1;
      reportProgress();
    });
    const failures = results.filter((result) => result.status === "rejected").length;
    if (failures) {
      throw new Error(`有 ${failures} 个文件下载失败（共 ${missing.length} 个）`);
    }
  } finally {
    const { [entry.id]: _, ...remaining } = downloadProgressByEntryId.value;
    downloadProgressByEntryId.value = remaining;
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
    showToast(`上传失败：${error instanceof Error ? error.message : String(error)}`, "error");
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
        showToast(`自动上传失败：${error instanceof Error ? error.message : String(error)}`, "error");
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
        let finished = 0;
        const reportProgress = () => {
          downloadProgressByEntryId.value = {
            ...downloadProgressByEntryId.value,
            [entry.id]: { finished, total: preparation.missing.length },
          };
        };
        reportProgress();
        const results = await mapWithConcurrency(
          preparation.missing,
          TRANSFER_CONCURRENCY,
          async (file) => {
            await client.downloadFileToSave(entry, {
              fileId: file.fileId,
              size: file.size,
            }, preparation.saveId);
            finished += 1;
            reportProgress();
          },
        );
        const failures = results.filter((result) => result.status === "rejected").length;
        if (failures) {
          throw new Error(`有 ${failures} 个文件下载失败（共 ${preparation.missing.length} 个）`);
        }
      }

      const saved = await invoke<number>("finish_save_entry", { saveId: preparation.saveId });
      saveId = undefined;
      if (saved > 0) showToast(`已保存 ${saved} 个文件`, "success");
    }
  } catch (error) {
    if (saveId) await invoke("cancel_save_entry", { saveId }).catch(() => undefined);
    showToast(`${isMobile.value ? "下载" : "另存为"}失败：${error instanceof Error ? error.message : String(error)}`, "error");
  } finally {
    const { [entry.id]: _, ...remaining } = downloadProgressByEntryId.value;
    downloadProgressByEntryId.value = remaining;
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
  let serverProtocol: ServerProtocol = value.serverProtocol === "https" ? "https" : "http";
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
      : 10,
    autoReceiveClipboard: value.autoReceiveClipboard !== false,
  };
}

async function persistSyncConfig(config: SyncConfig): Promise<void> {
  if (runningInTauri) await invoke("save_sync_config", { config });
  else window.localStorage.setItem(BROWSER_CONFIG_KEY, JSON.stringify(config));
}

function setSetupFields(config?: SyncConfig): void {
  setupServerAddress.value = config?.serverAddress || DEFAULT_SERVER_ADDRESS;
  setupServerProtocol.value = config?.serverProtocol || CONFIGURED_SERVER_PROTOCOL;
  setupUsername.value = config?.username || "";
  setupPassword.value = "";
  authMode.value = "login";
  serverFieldError.value = "";
  usernameFieldError.value = "";
  passwordFieldError.value = "";
}

function validateServerField(): string | undefined {
  try {
    const normalized = normalizeServerAddress(setupServerAddress.value);
    setupServerAddress.value = normalized;
    serverFieldError.value = "";
    return normalized;
  } catch (error) {
    serverFieldError.value = error instanceof Error ? error.message : String(error);
    return undefined;
  }
}

function validateUsernameField(): boolean {
  usernameFieldError.value = /^[a-zA-Z0-9_.-]{3,32}$/.test(setupUsername.value.trim())
    ? ""
    : "账号需为 3-32 位字母、数字或 _.-";
  return !usernameFieldError.value;
}

function validatePasswordField(): boolean {
  const length = setupPassword.value.length;
  passwordFieldError.value = length >= 6 && length <= 128 ? "" : "密码长度需为 6-128 位";
  return !passwordFieldError.value;
}

function switchAuthMode(mode: AuthMode): void {
  authMode.value = mode;
  setupError.value = "";
  usernameFieldError.value = "";
  passwordFieldError.value = "";
}

function openSettings(): void {
  if (!activeSyncConfig) return;
  autoUploadLimitMb.value = activeSyncConfig.autoUploadLimitMb;
  autoReceiveClipboard.value = activeSyncConfig.autoReceiveClipboard;
  resetQuickPasteShortcutDraft();
  recordingQuickPasteShortcut.value = false;
  settingsPage.value = "general";
  clearPasswordChangeFields();
  settingsError.value = "";
  settingsVisible.value = true;
}

function selectSettingsPage(page: SettingsPage): void {
  settingsPage.value = page;
  recordingQuickPasteShortcut.value = false;
  settingsError.value = "";
}

function shortcutKeyToken(event: KeyboardEvent): string {
  const { code } = event;
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  if (/^F\d{1,2}$/.test(code)) return code;
  const tokens: Record<string, string> = {
    Space: "Space",
    Enter: "Enter",
    Tab: "Tab",
    Backquote: "`",
    Minus: "-",
    Equal: "=",
    ArrowUp: "Up",
    ArrowDown: "Down",
    ArrowLeft: "Left",
    ArrowRight: "Right",
  };
  return tokens[code] ?? "";
}

function recordQuickPasteShortcut(event: KeyboardEvent): void {
  if (!recordingQuickPasteShortcut.value) return;
  event.preventDefault();
  event.stopPropagation();
  if (event.code === "Escape") {
    recordingQuickPasteShortcut.value = false;
    return;
  }
  const token = shortcutKeyToken(event);
  if (!token) return;

  const modifiers: string[] = [];
  if (event.ctrlKey || event.metaKey) modifiers.push("CommandOrControl");
  if (event.shiftKey) modifiers.push("Shift");
  if (event.altKey) modifiers.push("Alt");
  if (!modifiers.length && !/^F\d{1,2}$/.test(token)) {
    quickPasteShortcutStatus.value = {
      state: "error",
      message: "请至少按住 Ctrl、Alt 或 Command，再按一个主键",
    };
    return;
  }
  quickPasteShortcut.value = [...modifiers, token].join("+");
  quickPasteShortcutStatus.value = { state: "idle", message: "点击保存后生效" };
  recordingQuickPasteShortcut.value = false;
}

function selectQuickPasteShortcut(shortcut: string): void {
  quickPasteShortcut.value = shortcut;
  quickPasteShortcutStatus.value = { state: "idle", message: "点击保存后生效" };
}

async function checkAppUpdate(): Promise<void> {
  await checkForUpdate({ silent: false });
}

function closeSettings(): void {
  if (savingSettings.value || changingPassword.value) return;
  settingsVisible.value = false;
  settingsError.value = "";
  clearPasswordChangeFields();
  void nextTick(focusSearch);
}

function clearPasswordChangeFields(): void {
  currentPassword.value = "";
  newPassword.value = "";
  confirmNewPassword.value = "";
  passwordChangeError.value = "";
}

function validateNewPassword(): boolean {
  const length = newPassword.value.length;
  passwordChangeError.value = length >= 6 && length <= 128 ? "" : "新密码长度需为 6-128 位";
  return !passwordChangeError.value;
}

function validatePasswordConfirmation(): boolean {
  passwordChangeError.value = newPassword.value === confirmNewPassword.value ? "" : "两次输入的新密码不一致";
  return !passwordChangeError.value;
}

async function openAppDataDirectory(): Promise<void> {
  if (!runningInTauri) return;
  settingsError.value = "";
  try {
    await invoke("open_app_data_dir");
  } catch (error) {
    settingsError.value = `无法打开应用数据目录：${error instanceof Error ? error.message : String(error)}`;
  }
}

function closeSetup(): void {
  if (testingConnection.value || !hasSavedSyncConfig.value) return;
  setupVisible.value = false;
  setupError.value = "";
  void nextTick(focusSearch);
}

async function saveSettings(): Promise<void> {
  if (!activeSyncConfig || savingSettings.value || changingPassword.value) return;
  savingSettings.value = true;
  settingsError.value = "";
  const previousAutoUploadLimitMb = activeSyncConfig.autoUploadLimitMb;
  const config = {
    ...activeSyncConfig,
    autoUploadLimitMb: autoUploadLimitMb.value,
    autoReceiveClipboard: autoReceiveClipboard.value,
  };
  try {
    if (
      runningInTauri
      && platformCapabilities.value.globalShortcut
      && settingsPage.value === "shortcuts"
      && !(await saveQuickPasteShortcut())
    ) {
      settingsError.value = quickPasteShortcutStatus.value.message;
      return;
    }
    await persistSyncConfig(config);
    activeSyncConfig = config;
    if (config.enabled && config.username && config.sessionToken) await startSync(config);
    if (config.autoUploadLimitMb > previousAutoUploadLimitMb) {
      void uploadNowEligibleEntries(config.autoUploadLimitMb * 1024 * 1024);
    }
    settingsVisible.value = false;
    await nextTick();
    await focusSearch();
  } catch (error) {
    settingsError.value = `无法保存设置：${error instanceof Error ? error.message : String(error)}`;
  } finally {
    savingSettings.value = false;
  }
}

async function changePassword(): Promise<void> {
  if (!activeSyncConfig || !currentUsername.value || changingPassword.value || savingSettings.value) return;
  if (currentPassword.value.length < 6 || currentPassword.value.length > 128) {
    passwordChangeError.value = "请输入当前密码";
    return;
  }
  if (!validateNewPassword() || !validatePasswordConfirmation()) {
    return;
  }
  if (newPassword.value === currentPassword.value) {
    passwordChangeError.value = "新密码不能与当前密码相同";
    return;
  }

  changingPassword.value = true;
  settingsError.value = "";
  try {
    await changeAccountPassword(
      activeSyncConfig.serverAddress,
      activeSyncConfig.serverProtocol,
      activeSyncConfig.sessionToken,
      currentPassword.value,
      newPassword.value,
    );
    const config: SyncConfig = {
      ...activeSyncConfig,
      enabled: true,
      sessionToken: "",
    };
    await persistSyncConfig(config);
    activeSyncConfig = config;
    currentUsername.value = config.username;
    syncClient?.stop();
    syncClient = undefined;
    connected.value = false;
    syncEnabled.value = true;
    settingsVisible.value = false;
    clearPasswordChangeFields();
    setSetupFields(config);
    setupError.value = "密码已修改，请使用新密码重新登录";
    setupVisible.value = true;
    await nextTick();
    accountPasswordInput.value?.focus();
  } catch (error) {
    settingsError.value = `修改密码失败：${error instanceof Error ? error.message : String(error)}`;
  } finally {
    changingPassword.value = false;
  }
}

async function signOut(openLogin: boolean): Promise<void> {
  if (!activeSyncConfig || savingSettings.value || changingPassword.value) return;
  savingSettings.value = true;
  settingsError.value = "";
  const config: SyncConfig = {
    ...activeSyncConfig,
    enabled: false,
    username: "",
    sessionToken: "",
  };
  try {
    await persistSyncConfig(config);
    activeSyncConfig = config;
    currentUsername.value = "";
    syncClient?.stop();
    syncClient = undefined;
    connected.value = false;
    syncEnabled.value = false;
    settingsVisible.value = false;
    if (openLogin) {
      setSetupFields(config);
      setupVisible.value = true;
      await nextTick();
      serverInput.value?.focus();
    } else {
      await nextTick();
      await focusSearch();
    }
  } catch (error) {
    settingsError.value = `无法退出账号：${error instanceof Error ? error.message : String(error)}`;
  } finally {
    savingSettings.value = false;
  }
}

async function useLocalMode(): Promise<void> {
  if (testingConnection.value) return;
  const config: SyncConfig = {
    enabled: false,
    serverAddress: setupServerAddress.value.trim() || DEFAULT_SERVER_ADDRESS,
    serverProtocol: setupServerProtocol.value,
    username: setupUsername.value.trim(),
    sessionToken: activeSyncConfig?.sessionToken ?? "",
    autoUploadLimitMb: activeSyncConfig?.autoUploadLimitMb ?? 10,
    autoReceiveClipboard: activeSyncConfig?.autoReceiveClipboard ?? true,
  };
  setupError.value = "";
  try {
    await persistSyncConfig(config);
    activeSyncConfig = config;
    currentUsername.value = config.username;
    hasSavedSyncConfig.value = true;
    syncClient?.stop();
    syncClient = undefined;
    connected.value = false;
    syncEnabled.value = false;
    setupVisible.value = false;
    await refreshEntries();
    await nextTick();
    await focusSearch();
  } catch (error) {
    setupError.value = `无法保存设置：${error instanceof Error ? error.message : String(error)}`;
  }
}

async function connectAndSave(): Promise<void> {
  setupError.value = "";
  const serverAddress = validateServerField();
  const usernameValid = validateUsernameField();
  const passwordValid = validatePasswordField();
  if (!serverAddress || !usernameValid || !passwordValid) return;
  const username = setupUsername.value.trim();

  testingConnection.value = true;
  let accountCreated = false;
  try {
    const device = await getDevice();
    const session = await authenticateAccount(
      serverAddress,
      username,
      setupPassword.value,
      authMode.value,
      setupServerProtocol.value,
      device.id,
    );
    accountCreated = authMode.value === "register";
    const { webSocketUrl } = getServerUrls(serverAddress, setupServerProtocol.value);
    await testSyncConnection(webSocketUrl, session.sessionToken, device);
    const config: SyncConfig = {
      enabled: true,
      serverAddress,
      serverProtocol: setupServerProtocol.value,
      username: session.user.username,
      sessionToken: session.sessionToken,
      autoUploadLimitMb: 10,
      autoReceiveClipboard: true,
    };
    await persistSyncConfig(config);
    activeSyncConfig = config;
    currentUsername.value = config.username;
    hasSavedSyncConfig.value = true;
    setupServerAddress.value = serverAddress;
    setupServerProtocol.value = config.serverProtocol;
    setupUsername.value = session.user.username;
    setupPassword.value = "";
    setupVisible.value = false;
    await refreshEntries();
    await startSync(config);
    await nextTick();
    await focusSearch();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (accountCreated) {
      authMode.value = "login";
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

async function upsertRemote(entry: ClipboardEntry): Promise<void> {
  markEntrySynced(entry);
  if (!runningInTauri) {
    entries.value = [
      { ...entry, summary: EMPTY_SUMMARY },
      ...entries.value.filter((item) => item.id !== entry.id),
    ].sort((a, b) => b.createdAt.localeCompare(a.createdAt));
    return;
  }
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
      showToast(`自动接收剪贴板失败：${error instanceof Error ? error.message : String(error)}`, "error");
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
    showToast(`刷新上传状态失败：${error instanceof Error ? error.message : String(error)}`, "error");
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
      showToast(`同步历史失败：${error instanceof Error ? error.message : String(error)}`, "error");
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
        const { [entryId]: _, ...remaining } = uploadProgressByEntryId.value;
        uploadProgressByEntryId.value = remaining;
      },
      onError: (message) => { showToast(message, "error"); },
      onAuthenticationFailed: (message) => {
        if (syncClient !== client) return;
        client.stop();
        syncClient = undefined;
        connected.value = false;
        const alreadyRelogging = setupVisible.value && !activeSyncConfig?.sessionToken;
        const expiredConfig = { ...config, sessionToken: "" };
        activeSyncConfig = expiredConfig;
        currentUsername.value = expiredConfig.username;
        if (!alreadyRelogging) setSetupFields(expiredConfig);
        setupError.value = message;
        setupVisible.value = true;
        if (!alreadyRelogging) void persistSyncConfig(expiredConfig);
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
    syncClient?.stop();
    syncClient = undefined;
    connected.value = false;
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
  const message = error instanceof Error ? error.message : String(error);
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
    listen("cliproam://focus-search", focusSearch),
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
          message: error instanceof Error ? error.message : String(error),
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
      showToast(`系统分享接收初始化失败：${error instanceof Error ? error.message : String(error)}`, "error");
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
    setSetupFields(config);
    if (config.enabled && (!config.username || !config.sessionToken)) setupVisible.value = true;
  }
  initializing.value = false;

  if (startupWarning) showToast(startupWarning, "error");
  if (!isPasteWindow) void initUpdaterVersion();
  void refreshEntries().catch((error) => {
    showToast(`剪贴板历史读取失败：${error instanceof Error ? error.message : String(error)}`, "error");
  });
  if (runningInTauri) void initializeTauriServices();

  if (setupVisible.value) {
    await nextTick();
    serverInput.value?.focus();
  } else if (config?.enabled && config.username && config.sessionToken) {
    try {
      await startSync(config);
    } catch (error) {
      showToast(`同步初始化失败：${error instanceof Error ? error.message : String(error)}`, "error");
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
    <button
      v-if="hasSavedSyncConfig"
      class="icon-button setup-back-button"
      type="button"
      title="返回剪贴板历史"
      aria-label="返回剪贴板历史"
      :disabled="testingConnection"
      @click="closeSetup"
    >
      <ArrowLeft :size="17" aria-hidden="true" />
    </button>

    <section class="setup-content">
      <div class="setup-intro">
        <span class="setup-icon" aria-hidden="true"><Server :size="24" /></span>
        <span class="setup-eyebrow">{{ hasSavedSyncConfig ? "重新登录" : "首次设置" }}</span>
        <h1>{{ authMode === "login" ? "登录同步服务器" : "创建同步账号" }}</h1>
        <p>每个账号拥有独立的剪贴板内容和设备列表。</p>
      </div>

      <form class="setup-form" @submit.prevent="connectAndSave">
        <div class="auth-mode-switch" aria-label="账号操作">
          <button type="button" :class="{ active: authMode === 'login' }" :aria-pressed="authMode === 'login'" @click="switchAuthMode('login')">登录</button>
          <button type="button" :class="{ active: authMode === 'register' }" :aria-pressed="authMode === 'register'" @click="switchAuthMode('register')">注册</button>
        </div>

        <div class="server-connection-fields">
          <div class="server-address-field">
            <label for="server-address">服务器地址</label>
            <input
              id="server-address"
              ref="serverInput"
              v-model="setupServerAddress"
              type="text"
              inputmode="text"
              autocomplete="off"
              spellcheck="false"
              placeholder="192.168.1.20:4810"
              :disabled="testingConnection"
              :aria-invalid="Boolean(serverFieldError)"
              :aria-describedby="serverFieldError ? 'server-address-error' : 'server-connection-hint'"
              @blur="validateServerField"
            />
          </div>
          <div class="server-protocol-field">
            <label for="server-protocol">协议</label>
            <select id="server-protocol" v-model="setupServerProtocol" :disabled="testingConnection" aria-describedby="server-connection-hint">
              <option value="http">HTTP</option>
              <option value="https">HTTPS</option>
            </select>
          </div>
        </div>
        <span v-if="serverFieldError" id="server-address-error" class="field-error">{{ serverFieldError }}</span>
        <span v-else id="server-connection-hint" class="field-hint">
          {{ setupServerProtocol === "https"
            ? "HTTPS + WSS：服务端需要配置可信 TLS 证书。"
            : "HTTP + WS 未加密，仅应在受信任的网络中使用。" }}
        </span>

        <label for="account-username">账号</label>
        <input
          id="account-username"
          v-model="setupUsername"
          type="text"
          autocomplete="username"
          spellcheck="false"
          placeholder="请输入账号"
          :disabled="testingConnection"
          :aria-invalid="Boolean(usernameFieldError)"
          :aria-describedby="usernameFieldError ? 'account-username-error' : undefined"
          @blur="validateUsernameField"
        />
        <span v-if="usernameFieldError" id="account-username-error" class="field-error">{{ usernameFieldError }}</span>

        <label for="account-password">密码</label>
        <input
          id="account-password"
          ref="accountPasswordInput"
          v-model="setupPassword"
          type="password"
          :autocomplete="authMode === 'login' ? 'current-password' : 'new-password'"
          placeholder="请输入密码"
          :disabled="testingConnection"
          :aria-invalid="Boolean(passwordFieldError)"
          :aria-describedby="passwordFieldError ? 'account-password-error' : 'account-password-hint'"
          @blur="validatePasswordField"
        />
        <span v-if="passwordFieldError" id="account-password-error" class="field-error">{{ passwordFieldError }}</span>
        <span v-else id="account-password-hint" class="field-hint">密码长度至少 6 位</span>

        <p v-if="setupError" class="setup-error" role="alert">{{ setupError }}</p>

        <button class="primary-button" type="submit" :disabled="testingConnection">
          <LoaderCircle v-if="testingConnection" :size="17" class="spin" aria-hidden="true" />
          <ShieldCheck v-else :size="17" aria-hidden="true" />
          {{ testingConnection
            ? (authMode === "login" ? "正在登录…" : "正在创建账号…")
            : (authMode === "login" ? "登录并连接" : "创建账号并连接") }}
        </button>
        <button class="secondary-button" type="button" :disabled="testingConnection" @click="useLocalMode">
          暂时仅使用本地剪贴板
        </button>
      </form>
    </section>

    <footer class="setup-footer">
      当前设备仅保存登录会话，不保存账号密码
    </footer>
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

    <div v-if="!isPasteWindow && settingsVisible" class="settings-backdrop" @mousedown.self="closeSettings">
      <section class="settings-dialog" role="dialog" aria-modal="true" aria-labelledby="settings-heading">
        <header class="settings-dialog-header">
          <div>
            <span>设置</span>
            <h2 id="settings-heading">本机偏好</h2>
          </div>
          <button class="icon-button" type="button" title="关闭设置" aria-label="关闭设置" :disabled="savingSettings || changingPassword" @click="closeSettings">
            <X :size="17" />
          </button>
        </header>

        <div class="settings-layout">
          <nav class="settings-nav" aria-label="设置分类" role="tablist">
            <button :class="{ active: settingsPage === 'general' }" type="button" role="tab" aria-controls="settings-general-panel" :aria-selected="settingsPage === 'general'" @click="selectSettingsPage('general')">通用</button>
            <button v-if="runningInTauri && platformCapabilities.globalShortcut" :class="{ active: settingsPage === 'shortcuts' }" type="button" role="tab" aria-controls="settings-shortcuts-panel" :aria-selected="settingsPage === 'shortcuts'" @click="selectSettingsPage('shortcuts')">快捷键</button>
            <button :class="{ active: settingsPage === 'account' }" type="button" role="tab" aria-controls="settings-account-panel" :aria-selected="settingsPage === 'account'" @click="selectSettingsPage('account')">账号与安全</button>
            <button :class="{ active: settingsPage === 'data' }" type="button" role="tab" aria-controls="settings-data-panel" :aria-selected="settingsPage === 'data'" @click="selectSettingsPage('data')">应用数据</button>
            <button :class="{ active: settingsPage === 'about' }" type="button" role="tab" aria-controls="settings-about-panel" :aria-selected="settingsPage === 'about'" @click="selectSettingsPage('about')">关于</button>
          </nav>

          <form class="settings-form" @submit.prevent="saveSettings">
            <section v-if="settingsPage === 'general'" id="settings-general-panel" class="settings-page" role="tabpanel" aria-labelledby="general-settings-heading">
              <header class="settings-page-header">
                <h3 id="general-settings-heading">通用</h3>
                <p>配置当前设备的剪贴板漫游和文件同步行为。</p>
              </header>
              <section class="settings-section" aria-labelledby="roaming-settings-heading">
                <div class="settings-section-heading">
                  <span class="settings-icon" aria-hidden="true"><Clipboard :size="18" /></span>
                  <div>
                    <h4 id="roaming-settings-heading">剪贴板漫游</h4>
                    <p>其他在线设备复制内容后，直接更新本机系统剪贴板。</p>
                  </div>
                </div>
                <label class="setting-switch" for="auto-receive-clipboard">
                  <span class="setting-switch-copy">
                    <strong>自动接收剪贴板</strong>
                    <small>{{ isMobile
                      ? "移动端前台支持文本；图片和文件同步到历史后可手动下载。"
                      : "支持文本、富文本和图片；文件与文件夹只同步到历史，需手动选择粘贴。" }}</small>
                  </span>
                  <input id="auto-receive-clipboard" v-model="autoReceiveClipboard" type="checkbox" role="switch" :disabled="savingSettings" />
                  <span class="setting-switch-track" aria-hidden="true"></span>
                </label>
              </section>
              <section class="settings-section" aria-labelledby="upload-settings-heading">
                <div class="settings-section-heading">
                  <span class="settings-icon" aria-hidden="true"><FolderOpen :size="18" /></span>
                  <div>
                    <h4 id="upload-settings-heading">文件同步</h4>
                    <p>配置当前设备自动上传到同步服务的文件大小上限。</p>
                  </div>
                </div>
                <label for="auto-upload-limit">自动上传文件</label>
                <select id="auto-upload-limit" v-model.number="autoUploadLimitMb" :disabled="savingSettings">
                  <option :value="0">关闭自动上传</option>
                  <option :value="1">小于 1 MB</option>
                  <option :value="2">小于 2 MB</option>
                  <option :value="5">小于 5 MB</option>
                  <option :value="10">小于 10 MB</option>
                  <option :value="20">小于 20 MB</option>
                  <option :value="50">小于 50 MB</option>
                  <option :value="100">小于 100 MB</option>
                </select>
                <span class="field-hint">超过上限的文件不会自动上传，粘贴时需要源设备在线。</span>
              </section>
            </section>

            <section v-else-if="settingsPage === 'shortcuts'" id="settings-shortcuts-panel" class="settings-page" role="tabpanel" aria-labelledby="shortcuts-page-heading">
              <header class="settings-page-header">
                <h3 id="shortcuts-page-heading">快捷键</h3>
                <p>配置当前设备的全局快捷操作，不会同步到其他设备。</p>
              </header>
              <section class="settings-section" aria-labelledby="quick-paste-shortcut-heading">
                <div class="settings-section-heading">
                  <span class="settings-icon" aria-hidden="true"><KeyRound :size="18" /></span>
                  <div>
                    <h4 id="quick-paste-shortcut-heading">快捷粘贴</h4>
                    <p>在其他应用中按下快捷键，打开 ClipRoam 快捷粘贴窗口。</p>
                  </div>
                </div>
                <div class="shortcut-setting-row">
                  <div>
                    <strong>全局快捷键</strong>
                    <small>点击右侧按钮，然后按下新的组合键；Esc 取消录制。</small>
                  </div>
                  <button
                    class="shortcut-recorder"
                    :class="{ recording: recordingQuickPasteShortcut }"
                    type="button"
                    :disabled="savingSettings || quickPasteShortcutRefreshing"
                    :aria-label="recordingQuickPasteShortcut ? '正在录制快捷粘贴快捷键' : `当前快捷键 ${displayShortcut(quickPasteShortcut)}`"
                    @click="recordingQuickPasteShortcut = true"
                    @blur="recordingQuickPasteShortcut = false"
                    @keydown="recordQuickPasteShortcut"
                  >
                    {{ recordingQuickPasteShortcut ? "按下组合键…" : displayShortcut(quickPasteShortcut) }}
                  </button>
                </div>
                <div class="shortcut-presets" aria-label="快捷键预设">
                  <span>预设</span>
                  <button
                    v-for="preset in ['CommandOrControl+Shift+V', 'CommandOrControl+Alt+V', 'CommandOrControl+Shift+Space']"
                    :key="preset"
                    type="button"
                    :class="{ active: quickPasteShortcut === preset }"
                    :disabled="savingSettings || quickPasteShortcutRefreshing"
                    @click="selectQuickPasteShortcut(preset)"
                  >
                    {{ displayShortcut(preset) }}
                  </button>
                </div>
                <p v-if="quickPasteShortcutStatus.message" class="shortcut-status" :class="quickPasteShortcutStatus.state" :role="quickPasteShortcutStatus.state === 'error' ? 'alert' : 'status'" aria-live="polite">
                  {{ quickPasteShortcutStatus.message }}
                </p>
              </section>
            </section>

            <section v-else-if="settingsPage === 'account'" id="settings-account-panel" class="settings-page" role="tabpanel" aria-labelledby="account-page-heading">
              <header class="settings-page-header">
                <h3 id="account-page-heading">账号与安全</h3>
                <p>管理同步账号和登录安全。</p>
              </header>
              <section class="settings-section" aria-labelledby="account-settings-heading">
                <div class="settings-section-heading">
                  <span class="settings-icon" aria-hidden="true"><Cloud :size="18" /></span>
                  <div>
                    <h4 id="account-settings-heading">账号</h4>
                    <p>{{ currentUsername ? `当前登录：${currentUsername}` : "当前未登录同步账号" }}</p>
                  </div>
                </div>
                <div class="account-actions">
                  <button class="secondary-button" type="button" :disabled="savingSettings || changingPassword" @click="signOut(true)">切换账号</button>
                  <button class="danger-button" type="button" :disabled="savingSettings || changingPassword || !currentUsername" @click="signOut(false)">退出账号</button>
                </div>
              </section>

              <section v-if="syncEnabled && currentUsername" class="settings-section" aria-labelledby="password-settings-heading">
                <div class="settings-section-heading">
                  <span class="settings-icon" aria-hidden="true"><ShieldCheck :size="18" /></span>
                  <div>
                    <h4 id="password-settings-heading">修改密码</h4>
                    <p>修改后，所有设备需要使用新密码重新登录。</p>
                  </div>
                </div>
                <div class="password-change-fields">
                  <label for="current-password">当前密码</label>
                  <input id="current-password" v-model="currentPassword" type="password" autocomplete="current-password" :disabled="savingSettings || changingPassword" />
                  <label for="new-password">新密码</label>
                  <input id="new-password" v-model="newPassword" type="password" autocomplete="new-password" minlength="6" maxlength="128" placeholder="至少 6 位" :aria-invalid="Boolean(passwordChangeError)" :aria-describedby="passwordChangeError ? 'password-change-error' : 'password-change-hint'" :disabled="savingSettings || changingPassword" @blur="validateNewPassword" />
                  <label for="confirm-new-password">确认新密码</label>
                  <input id="confirm-new-password" v-model="confirmNewPassword" type="password" autocomplete="new-password" minlength="6" maxlength="128" :aria-invalid="Boolean(passwordChangeError)" :aria-describedby="passwordChangeError ? 'password-change-error' : 'password-change-hint'" :disabled="savingSettings || changingPassword" @blur="validatePasswordConfirmation" />
                </div>
                <span v-if="passwordChangeError" id="password-change-error" class="field-error" role="alert">{{ passwordChangeError }}</span>
                <span v-else id="password-change-hint" class="field-hint">新密码长度为 6-128 位。</span>
                <button class="secondary-button" type="button" :disabled="savingSettings || changingPassword" @click="changePassword">
                  <LoaderCircle v-if="changingPassword" :size="17" class="spin" aria-hidden="true" />
                  {{ changingPassword ? "正在修改…" : "修改密码" }}
                </button>
              </section>
            </section>

            <section v-else-if="settingsPage === 'data'" id="settings-data-panel" class="settings-page" role="tabpanel" aria-labelledby="data-page-heading">
              <header class="settings-page-header">
                <h3 id="data-page-heading">应用数据</h3>
                <p>查看当前设备保存的历史和配置文件。</p>
              </header>
              <section class="settings-section" aria-labelledby="data-settings-heading">
                <div class="settings-section-heading">
                  <span class="settings-icon" aria-hidden="true"><FolderOpen :size="18" /></span>
                  <div>
                    <h4 id="data-settings-heading">本地数据目录</h4>
                    <p>{{ isMobile
                      ? "移动端数据保存在系统应用沙箱中，卸载应用时会一并移除。"
                      : "包含本地剪贴板历史、同步配置和已保存的文件。" }}</p>
                  </div>
                </div>
                <button v-if="platformCapabilities.openDataDirectory" class="secondary-button" type="button" :disabled="savingSettings || changingPassword" @click="openAppDataDirectory">打开应用数据</button>
              </section>
            </section>

            <section v-else id="settings-about-panel" class="settings-page" role="tabpanel" aria-labelledby="about-page-heading">
              <header class="settings-page-header">
                <h3 id="about-page-heading">关于</h3>
                <p>查看应用版本和更新状态。</p>
              </header>

              <section class="settings-section about-product" aria-labelledby="about-product-heading">
                <img class="about-product-mark" src="/cliproam-icon.png" alt="" />
                <div class="about-product-copy">
                  <h4 id="about-product-heading">ClipRoam</h4>
                  <p>让剪贴板内容在你的设备之间安全漫游。</p>
                </div>
                <span class="about-version">v{{ appVersion || "…" }}</span>
              </section>

              <section class="settings-section about-update" aria-labelledby="update-settings-heading">
                <div class="settings-section-heading">
                  <span class="settings-icon" aria-hidden="true"><RefreshCw :size="18" /></span>
                  <div>
                    <h4 id="update-settings-heading">应用更新</h4>
                    <p>检查 GitHub Release 中是否有可用的新版本。</p>
                  </div>
                </div>
                <p class="about-update-status" :class="{ 'update-error': updateStatus === 'error' }" :role="updateStatus === 'error' ? 'alert' : 'status'" aria-live="polite">
                  {{ updateStatusText }}
                </p>
                <button
                  class="secondary-button about-update-button"
                  type="button"
                  :disabled="!updaterSupported || updateStatus === 'checking' || updateStatus === 'downloading'"
                  @click="checkAppUpdate"
                >
                  <LoaderCircle v-if="updateStatus === 'checking'" :size="17" class="spin" aria-hidden="true" />
                  <RefreshCw v-else :size="17" aria-hidden="true" />
                  {{ !updaterSupported
                    ? "当前平台不支持"
                    : updateStatus === "checking"
                      ? "检查中…"
                      : updateStatus === "downloading"
                        ? "下载中…"
                        : "检查更新" }}
                </button>
              </section>
            </section>

            <p v-if="settingsError" class="setup-error" role="alert">{{ settingsError }}</p>

            <footer v-if="settingsPage === 'general' || settingsPage === 'shortcuts'" class="settings-actions">
              <button class="primary-button" type="submit" :disabled="savingSettings || changingPassword">
                <LoaderCircle v-if="savingSettings" :size="17" class="spin" aria-hidden="true" />
                <Check v-else :size="17" aria-hidden="true" />
                {{ savingSettings ? "正在保存…" : "保存设置" }}
              </button>
            </footer>
          </form>
        </div>
      </section>
    </div>

  </main>

  <UpdaterDialog v-if="!isPasteWindow && !isToastWindow" locale="zh-CN" />

  <ToastLayer />
</template>
