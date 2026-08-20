<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { addPluginListener, convertFileSrc, invoke, type PluginListener } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { UpdaterDialog } from "@while-coder/tauri-updater-vue";
import type {
  ClipboardEntry,
  ClipboardKind,
  ClipboardManifestEntry,
  Device,
} from "@cliproam/protocol";
import {
  Check,
  CircleAlert,
  CircleCheck,
  Clipboard,
  Cloud,
  CloudOff,
  Download,
  File,
  FileText,
  FolderOpen,
  Image,
  Info,
  LoaderCircle,
  Monitor,
  Pin,
  RefreshCw,
  Search,
  Server,
  Settings2,
  ShieldCheck,
  Trash2,
  Upload,
  X,
} from "lucide-vue-next";
import {
  MANUAL_UPLOAD_LIMIT,
  SyncClient,
  authenticateAccount,
  changeAccountPassword,
  getServerUrls,
  normalizeServerAddress,
  testSyncConnection,
  type AuthMode,
  type ServerProtocol,
} from "./services/syncClient";
import { mapWithConcurrency, TRANSFER_CONCURRENCY } from "./services/concurrency";
import { useUpdater } from "./useUpdater";

const HOTKEY = "CommandOrControl+Shift+V";
const CONFIGURED_SERVER_ADDRESS = "127.0.0.1:4810";
const CONFIGURED_SERVER_PROTOCOL = "http";
const DEFAULT_SERVER_ADDRESS = CONFIGURED_SERVER_ADDRESS.includes("://")
  ? new URL(CONFIGURED_SERVER_ADDRESS).host
  : CONFIGURED_SERVER_ADDRESS;
const BROWSER_CONFIG_KEY = "cliproam.syncConfig";
const runningInTauri = "__TAURI_INTERNALS__" in window;
const isPasteWindow = runningInTauri && getCurrentWindow().label === "paste";
const isFeedbackWindow = runningInTauri && getCurrentWindow().label === "feedback";
if (isFeedbackWindow) document.documentElement.classList.add("feedback-window-root");

const {
  appVersion,
  updaterSupported,
  updateStatus,
  updateStatusText,
  checkForUpdate,
  initUpdaterVersion,
} = useUpdater();

type SyncConfig = {
  enabled: boolean;
  serverAddress: string;
  serverProtocol: ServerProtocol;
  username: string;
  sessionToken: string;
  autoUploadLimitMb: number;
  autoReceiveClipboard: boolean;
};

type MissingFile = { fileId: string; size: number; sourceDeviceId: string };
type SavePreparation = { saveId: string; missing: MissingFile[] };
type VirtualFileRequest = {
  entryId: string;
  fileId: string;
  size: number;
  sourceDeviceId: string;
};

type PlatformCapabilities = {
  mobile: boolean;
  clipboardMonitoring: boolean;
  globalShortcut: boolean;
  automaticPaste: boolean;
  fileClipboard: boolean;
  imageClipboard: boolean;
  nativeFileExport: boolean;
  openDataDirectory: boolean;
  shareReceiver: boolean;
};

const DESKTOP_CAPABILITIES: PlatformCapabilities = {
  mobile: false,
  clipboardMonitoring: true,
  globalShortcut: true,
  automaticPaste: true,
  fileClipboard: true,
  imageClipboard: true,
  nativeFileExport: true,
  openDataDirectory: true,
  shareReceiver: false,
};

/**
 * Aggregates computed by the backend. A folder can hold thousands of nodes, so
 * the list never receives the tree itself — only these counters.
 */
type EntrySummary = {
  rootKind: string;
  fileCount: number;
  hashedCount: number;
  contentCount: number;
  totalSize: number;
  maxFileSize: number;
  uploadedCount: number;
  readyCount: number;
  pendingCount: number;
  pendingSize: number;
  uploadableSize?: number;
  previewPath?: string;
};

type LocalClipboardEntry = ClipboardEntry & { summary: EntrySummary };
type UploadProgress = { uploadedBytes: number; totalBytes: number };
type DownloadProgress = { finished: number; total: number };

const EMPTY_SUMMARY: EntrySummary = {
  rootKind: "",
  fileCount: 0,
  hashedCount: 0,
  contentCount: 0,
  totalSize: 0,
  maxFileSize: 0,
  uploadedCount: 0,
  readyCount: 0,
  pendingCount: 0,
  pendingSize: 0,
};
type SettingsPage = "general" | "account" | "data" | "about";
type FeedbackTone = "success" | "error" | "info";
type FeedbackPayload = { message: string; tone: FeedbackTone };
type ShareReceiverEvent = { id?: string; error?: string };
type ShareImportSummary = { shares: number; texts: number; images: number; files: number };

const entries = ref<LocalClipboardEntry[]>([]);
const syncedEntryIds = ref(new Set<string>());
const devicesById = ref<Record<string, Device>>({
  browser: { id: "browser", name: "浏览器预览", platform: "browser", osVersion: "未知" },
});
const currentTime = ref(Date.now());
const query = ref("");
const filter = ref<"all" | ClipboardKind | "pending-upload">("all");
const selectedEntryId = ref("");
const connected = ref(false);
const syncEnabled = ref(false);
const feedbackToast = ref<FeedbackPayload>();
const platformCapabilities = ref<PlatformCapabilities>(DESKTOP_CAPABILITIES);
const isMobile = computed(() => platformCapabilities.value.mobile);
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
const changingPassword = ref(false);
const settingsError = ref("");
const passwordChangeError = ref("");
const currentPassword = ref("");
const newPassword = ref("");
const confirmNewPassword = ref("");
const currentUsername = ref("");
const capturingClipboard = ref(false);
const importingShare = ref(false);
const activatingEntryId = ref("");
const uploadingEntryId = ref("");
const uploadProgressByEntryId = ref<Record<string, UploadProgress>>({});
const downloadProgressByEntryId = ref<Record<string, DownloadProgress>>({});
const savingEntryId = ref("");
const previewImage = ref<LocalClipboardEntry>();
const previewLoading = ref(false);
const previewDialog = ref<HTMLElement>();
const searchInput = ref<HTMLInputElement>();
const serverInput = ref<HTMLInputElement>();
const accountPasswordInput = ref<HTMLInputElement>();
let activeSyncConfig: SyncConfig | undefined;
let syncClient: SyncClient | undefined;
let unlisteners: UnlistenFn[] = [];
let ageRefreshTimer: number | undefined;
let feedbackTimer: number | undefined;
let feedbackWindowHideTimer: number | undefined;
let globalShortcutApi: typeof import("@tauri-apps/plugin-global-shortcut") | undefined;
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
    tree: undefined,
    files: [],
    sourceDeviceId: "browser",
    createdAt: new Date().toISOString(),
    pinned: true,
    summary: EMPTY_SUMMARY,
  },
];

const filteredEntries = computed(() => {
  const needle = query.value.trim().toLocaleLowerCase();
  return entries.value.filter((entry) => {
    const matchesType = filter.value === "all"
      || (filter.value === "pending-upload" && entry.summary.uploadedCount < entry.summary.contentCount)
      || entry.kind === filter.value;
    const matchesQuery = !needle
      || entry.content.toLocaleLowerCase().includes(needle)
      || deviceName(entry).toLocaleLowerCase().includes(needle);
    return matchesType && matchesQuery;
  });
});

watch(filteredEntries, (nextEntries) => {
  if (!nextEntries.some((entry) => entry.id === selectedEntryId.value)) {
    selectedEntryId.value = nextEntries[0]?.id ?? "";
  }
});

const selectedIndex = computed(() => {
  const index = filteredEntries.value.findIndex((entry) => entry.id === selectedEntryId.value);
  return index >= 0 ? index : 0;
});

function displayFeedback(payload: FeedbackPayload): void {
  if (feedbackTimer !== undefined) window.clearTimeout(feedbackTimer);
  if (feedbackWindowHideTimer !== undefined) window.clearTimeout(feedbackWindowHideTimer);
  feedbackToast.value = payload;
  feedbackTimer = window.setTimeout(() => {
    feedbackToast.value = undefined;
    feedbackTimer = undefined;
    if (isFeedbackWindow) {
      feedbackWindowHideTimer = window.setTimeout(() => {
        void invoke("hide_feedback");
        feedbackWindowHideTimer = undefined;
      }, 180);
    }
  }, payload.tone === "error" ? 5_000 : 3_200);
}

function showFeedback(message: string, tone: FeedbackTone = "info"): void {
  const normalized = message.trim();
  if (!normalized) return;
  const payload = { message: normalized, tone };
  if (!runningInTauri) {
    displayFeedback(payload);
    return;
  }
  void invoke("show_feedback", payload).catch(() => displayFeedback(payload));
}

function formatAge(createdAt: string): string {
  const elapsed = Math.max(0, currentTime.value - new Date(createdAt).getTime());
  const seconds = Math.floor(elapsed / 1_000);
  if (seconds < 10) return "刚刚";
  if (seconds < 60) return `${seconds} 秒前`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  return new Intl.DateTimeFormat("zh-CN", { month: "short", day: "numeric" }).format(new Date(createdAt));
}

async function refreshEntries(): Promise<void> {
  if (!runningInTauri) {
    entries.value = demoEntries;
    return;
  }
  entries.value = await invoke<LocalClipboardEntry[]>("list_entries");
}

function rememberDevices(devices: Device[]): void {
  devicesById.value = {
    ...devicesById.value,
    ...Object.fromEntries(devices.map((device) => [device.id, device])),
  };
}

function deviceName(entry: ClipboardEntry): string {
  return devicesById.value[entry.sourceDeviceId]?.name ?? "未知设备";
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

function syncStatusLabel(entry: ClipboardEntry): string {
  return isEntrySynced(entry) ? "已同步到服务器" : "未同步到服务器";
}

function imageSource(entry: LocalClipboardEntry): string | undefined {
  const path = entry.summary.previewPath;
  return path && runningInTauri ? convertFileSrc(path) : undefined;
}

function thumbnailSource(entry: LocalClipboardEntry): string | undefined {
  return entry.thumbnail ? `data:image/webp;base64,${entry.thumbnail}` : undefined;
}

function fileEntrySummary(entry: LocalClipboardEntry): string | undefined {
  if (entry.kind !== "files" || !entry.summary.fileCount) return undefined;
  const count = `${entry.summary.fileCount} 个文件`;
  if (!entry.summary.totalSize) return count;
  return `${count} · ${formatFileSize(entry.summary.totalSize)}`;
}

function formatFileSize(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

async function focusSearch(): Promise<void> {
  query.value = "";
  if (isPasteWindow) filter.value = "all";
  selectedEntryId.value = filteredEntries.value[0]?.id ?? "";
  await nextTick();
  searchInput.value?.focus();
}

async function captureCurrentClipboard(): Promise<void> {
  if (!runningInTauri || capturingClipboard.value) return;
  capturingClipboard.value = true;
  try {
    const captured = await invoke<boolean>("capture_current_clipboard_text");
    await refreshEntries();
    showFeedback(captured ? "已读取当前文本剪贴板" : "当前剪贴板没有可读取的文本", captured ? "success" : "info");
  } catch (error) {
    showFeedback(`读取剪贴板失败：${error instanceof Error ? error.message : String(error)}`, "error");
  } finally {
    capturingClipboard.value = false;
  }
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
    showFeedback(shareImportMessage(summary), "success");
  } catch (error) {
    showFeedback(`接收系统分享失败：${error instanceof Error ? error.message : String(error)}，请重新分享`, "error");
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
        available: true,
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
    showFeedback("已复制到系统剪贴板", "success");
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
    showFeedback(command === "copy_entry" ? "已复制到系统剪贴板" : "已粘贴到当前应用", "success");
  } catch (error) {
    if (String(error).includes("clipboard entry was not found")) {
      await refreshEntries();
      return;
    }
    showFeedback(String(error), "error");
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

function isHashing(entry: LocalClipboardEntry): boolean {
  return entry.summary.hashedCount < entry.summary.fileCount;
}

function uploadStatus(entry: LocalClipboardEntry): string | undefined {
  const summary = entry.summary;
  if (!summary.fileCount) return undefined;
  // Content ids are computed in the background, so a fresh entry is usable
  // locally before it can be addressed on the server.
  if (isHashing(entry)) return `计算中 ${summary.hashedCount}/${summary.fileCount}`;
  const download = downloadProgressByEntryId.value[entry.id];
  if (download) return `下载中 ${download.finished}/${download.total}`;
  const progress = uploadProgressByEntryId.value[entry.id];
  if (progress) {
    const percent = progress.totalBytes
      ? Math.min(99, Math.floor((progress.uploadedBytes / progress.totalBytes) * 100))
      : 0;
    return `上传中 ${percent}%`;
  }
  if (!summary.contentCount) return undefined;
  if (summary.uploadedCount === summary.contentCount) return "已上传";
  if (summary.uploadedCount) {
    return `部分上传（${summary.uploadedCount}/${summary.contentCount}）`;
  }
  if (summary.uploadableSize !== undefined && summary.uploadableSize >= MANUAL_UPLOAD_LIMIT) {
    return "未上传（超过 100 MB）";
  }
  return "未上传";
}

function canManualUpload(entry: LocalClipboardEntry): boolean {
  const uploadableSize = entry.summary.uploadableSize;
  return runningInTauri
    && !isHashing(entry)
    && uploadableSize !== undefined
    && uploadableSize < MANUAL_UPLOAD_LIMIT;
}

async function uploadEntry(entry: LocalClipboardEntry): Promise<void> {
  if (!syncClient) {
    showFeedback("同步服务未连接，无法上传文件", "error");
    return;
  }
  if (uploadingEntryId.value || uploadProgressByEntryId.value[entry.id] || !canManualUpload(entry)) return;
  uploadingEntryId.value = entry.id;
  try {
    // Publishing needs the tree, which the rendered list does not carry.
    await syncClient.upload(await fullEntry(entry));
  } catch (error) {
    showFeedback(`上传失败：${error instanceof Error ? error.message : String(error)}`, "error");
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
      await client.publish(await fullEntry(entry));
    } catch (error) {
      if (syncClient === client) {
        showFeedback(`自动上传失败：${error instanceof Error ? error.message : String(error)}`, "error");
      }
    }
  }
  if (syncClient === client) await refreshEntries();
}

function canSaveEntry(entry: LocalClipboardEntry): boolean {
  return runningInTauri
    && (entry.kind === "files" || entry.kind === "image")
    && entry.summary.contentCount > 0;
}

function saveEntryLabel(entry: LocalClipboardEntry): string {
  if (savingEntryId.value === entry.id) return isMobile.value ? "正在下载…" : "正在另存为…";
  return isMobile.value ? "下载到本机缓存" : "另存为…";
}

async function saveEntry(entry: LocalClipboardEntry): Promise<void> {
  if (savingEntryId.value || !canSaveEntry(entry)) return;
  savingEntryId.value = entry.id;
  let saveId: string | undefined;
  try {
    if (isMobile.value) {
      await ensureLocalFiles(entry);
      showFeedback("内容已下载到应用缓存，可在 ClipRoam 中离线使用", "success");
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
              available: true,
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
      if (saved > 0) showFeedback(`已保存 ${saved} 个文件`, "success");
    }
  } catch (error) {
    if (saveId) await invoke("cancel_save_entry", { saveId }).catch(() => undefined);
    showFeedback(`${isMobile.value ? "下载" : "另存为"}失败：${error instanceof Error ? error.message : String(error)}`, "error");
  } finally {
    const { [entry.id]: _, ...remaining } = downloadProgressByEntryId.value;
    downloadProgressByEntryId.value = remaining;
    savingEntryId.value = "";
  }
}

function selectOrActivate(entry: LocalClipboardEntry): void {
  selectedEntryId.value = entry.id;
  if (isPasteWindow) void pasteEntry(entry);
  else if (isMobile.value) void copyEntry(entry);
}

function activateSelectedEntry(entry?: LocalClipboardEntry): void {
  if (!entry) return;
  if (isPasteWindow) void pasteEntry(entry);
  else if (entry.kind === "files") {
    showFeedback("文件请使用 Ctrl+Shift+V 快捷粘贴，或点击“另存为…”手动下载", "info");
  } else {
    void copyEntry(entry);
  }
}

function moveSelection(offset: -1 | 1): void {
  if (!filteredEntries.value.length) return;
  const index = Math.min(
    Math.max(selectedIndex.value + offset, 0),
    filteredEntries.value.length - 1,
  );
  selectedEntryId.value = filteredEntries.value[index].id;
}

async function openImagePreview(entry: LocalClipboardEntry): Promise<void> {
  if (isPasteWindow || previewLoading.value) return;
  previewLoading.value = true;
  try {
    const localEntry = await ensureLocalFiles(entry);
    if (!imageSource(localEntry)) throw new Error("图片文件不可用");
    previewImage.value = localEntry;
    await nextTick();
    previewDialog.value?.focus();
  } catch (error) {
    showFeedback(`无法预览图片：${error instanceof Error ? error.message : String(error)}`, "error");
  } finally {
    previewLoading.value = false;
  }
}

function closeImagePreview(): void {
  previewImage.value = undefined;
  void nextTick(() => searchInput.value?.focus());
}

async function togglePin(entry: ClipboardEntry): Promise<void> {
  if (!runningInTauri) {
    entry.pinned = !entry.pinned;
    return;
  }
  await invoke("set_pinned", { entryId: entry.id, pinned: !entry.pinned });
  await refreshEntries();
  const client = syncClient;
  if (client) client.publishMetadata(await fullEntry(entry as LocalClipboardEntry));
}

async function removeEntry(entry: ClipboardEntry): Promise<void> {
  if (runningInTauri) await invoke("delete_entry", { entryId: entry.id });
  else entries.value = entries.value.filter((item) => item.id !== entry.id);
  syncClient?.delete(entry.id);
  await refreshEntries();
}

async function clearHistory(): Promise<void> {
  if (runningInTauri) await invoke("clear_history");
  else entries.value = entries.value.filter((entry) => entry.pinned);
  await refreshEntries();
}

async function startWindowDrag(event: MouseEvent): Promise<void> {
  if (!runningInTauri || isMobile.value || event.button !== 0) return;
  const target = event.target as HTMLElement;
  if (target.closest("button, input, [role='button']")) return;
  await invoke("start_window_drag");
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
  settingsPage.value = "general";
  clearPasswordChangeFields();
  settingsError.value = "";
  settingsVisible.value = true;
}

function selectSettingsPage(page: SettingsPage): void {
  settingsPage.value = page;
  settingsError.value = "";
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
  if (!isPasteWindow && previewImage.value) {
    if (event.key === "Escape") {
      event.preventDefault();
      closeImagePreview();
    }
    return;
  }
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
  if (event.key === "Escape") {
    event.preventDefault();
    void hideWindow();
  } else if (event.key === "ArrowDown") {
    event.preventDefault();
    moveSelection(1);
  } else if (event.key === "ArrowUp") {
    event.preventDefault();
    moveSelection(-1);
  } else if (event.key === "Enter" && !event.shiftKey) {
    event.preventDefault();
    activateSelectedEntry(filteredEntries.value[selectedIndex.value]);
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
  await invoke("upsert_remote_entry", { entry });
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
      showFeedback(`自动接收剪贴板失败：${error instanceof Error ? error.message : String(error)}`, "error");
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
    const pendingDeletions = runningInTauri
      ? new Set(await invoke<string[]>("list_pending_deletions"))
      : new Set<string>();
    for (const entryId of pendingDeletions) client.delete(entryId);
    syncedEntryIds.value = new Set(
      manifest.filter((entry) => !pendingDeletions.has(entry.id)).map((entry) => entry.id),
    );
    // Read the durable history rather than the rendered list. The latter can
    // be stale while another Tauri window is refreshing it.
    const localEntries = runningInTauri
      ? await invoke<LocalClipboardEntry[]>("list_entries")
      : [...entries.value];
    const localClientIds = new Set(localEntries.map((entry) => entry.id));
    const serverClientIds = new Set(manifest.map((entry) => entry.id));
    const remoteOnlyEntryIds = manifest
      .filter((entry) => !localClientIds.has(entry.id) && !pendingDeletions.has(entry.id))
      .map((entry) => entry.id);
    const remoteEntries = await client.fetchEntries(remoteOnlyEntryIds);

    for (const entry of remoteEntries) {
      if (syncClient !== client) return;
      await upsertRemote(entry);
    }

    for (const entry of localEntries) {
      if (pendingDeletions.has(entry.id) || serverClientIds.has(entry.id) || syncClient !== client) continue;
      // An entry whose contents are still being hashed has no addressable ids
      // yet; `entry-ready` publishes it once they exist.
      if (isHashing(entry)) continue;
      try {
        await client.restore(await fullEntry(entry));
      } catch (error) {
        // Another window can delete or replace this record after list_entries()
        // captured its snapshot. It no longer needs restoring.
        if (String(error).includes("剪贴板记录不存在")) continue;
        throw error;
      }
    }
    if (runningInTauri) {
      const pendingUpdates = await invoke<string[]>("list_pending_entry_updates");
      for (const entryId of pendingUpdates) {
        if (pendingDeletions.has(entryId) || syncClient !== client) continue;
        try {
          client.publishMetadata(await fullEntry({ id: entryId }));
        } catch (error) {
          if (!String(error).includes("剪贴板记录不存在")) throw error;
        }
      }
    }
    await refreshEntries();
  } catch (error) {
    if (syncClient === client) {
      showFeedback(`同步历史失败：${error instanceof Error ? error.message : String(error)}`, "error");
    }
  }
}

async function startSync(config: SyncConfig): Promise<void> {
  syncClient?.stop();
  connected.value = false;
  syncEnabled.value = true;
  const device = await getDevice();
  const { webSocketUrl } = getServerUrls(config.serverAddress, config.serverProtocol);
  let client: SyncClient;
  client = new SyncClient(
    webSocketUrl,
    config.sessionToken,
    device,
    {
      onConnected: (value) => {
        connected.value = value;
      },
      onManifest: (manifest, devices) => {
        rememberDevices(devices);
        void reconcileManifest(manifest);
      },
      onDevicePresence: (device) => { rememberDevices([device]); },
      onEntry: (entry) => {
        void upsertRemote(entry).then(() => {
          if (runningInTauri) return invoke("acknowledge_entry_update", { entryId: entry.id });
        });
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
          ]).then(refreshEntries);
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
      onError: (message) => { showFeedback(message, "error"); },
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

onMounted(async () => {
  if (isFeedbackWindow) {
    unlisteners = [await listen<FeedbackPayload>("cliproam://feedback", ({ payload }) => {
      displayFeedback(payload);
    })];
    return;
  }
  if (runningInTauri) {
    platformCapabilities.value = await invoke<PlatformCapabilities>("get_platform_capabilities");
  }
  if (!isPasteWindow && !isFeedbackWindow) await initUpdaterVersion();
  await refreshEntries();
  ageRefreshTimer = window.setInterval(() => { currentTime.value = Date.now(); }, 10_000);
  document.addEventListener("keydown", handleKeys);

  if (runningInTauri) {
    unlisteners = await Promise.all([
      listen<FeedbackPayload>("cliproam://feedback", ({ payload }) => {
        displayFeedback(payload);
      }),
      listen("cliproam://entry-created", () => {
        localClipboardRevision += 1;
        void refreshEntries();
      }),
      // Emitted once every content of an entry has a known id, which for a
      // folder happens after background hashing finishes.
      listen<string>("cliproam://entry-ready", async ({ payload }) => {
        await refreshEntries();
        if (isPasteWindow || !syncClient) return;
        try {
          await syncClient.publish(await invoke<ClipboardEntry>("get_entry", { entryId: payload }));
        } catch (error) {
          showFeedback(`自动上传失败：${error instanceof Error ? error.message : String(error)}`, "error");
        }
      }),
      listen("cliproam://history-changed", refreshEntries),
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
          await refreshEntries();
        } catch (error) {
          await invoke("fail_virtual_file_request", {
            fileId: payload.fileId,
            message: error instanceof Error ? error.message : String(error),
          }).catch(() => undefined);
        }
      }),
    ]);
    if (!isPasteWindow && platformCapabilities.value.globalShortcut) {
      globalShortcutApi = await import("@tauri-apps/plugin-global-shortcut");
    }
    if (!isPasteWindow && platformCapabilities.value.shareReceiver) {
      shareReceiverListener = await addPluginListener<ShareReceiverEvent>(
        "cliproam-share-receiver",
        "received",
        (payload) => {
          if (payload.error) {
            showFeedback(`接收系统分享失败：${payload.error}，请重新分享`, "error");
          } else {
            void consumeMobileShares();
          }
        },
      );
      await consumeMobileShares();
    }
    if (globalShortcutApi && !(await globalShortcutApi.isRegistered(HOTKEY))) {
      await globalShortcutApi.register(HOTKEY, (event) => {
        if (event.state === "Pressed") void invoke("open_paste");
      });
    }
  }

  let config: SyncConfig | null = null;
  try {
    config = await loadSyncConfig();
  } catch (error) {
    setupError.value = `无法读取连接设置：${error instanceof Error ? error.message : String(error)}`;
  }
  initializing.value = false;
  if (!config) {
    if (isPasteWindow) return;
    setupVisible.value = true;
    await nextTick();
    serverInput.value?.focus();
    return;
  }

  activeSyncConfig = config;
  syncEnabled.value = config.enabled;
  currentUsername.value = config.username;
  hasSavedSyncConfig.value = true;
  setSetupFields(config);
  if (config.enabled && config.username && config.sessionToken) {
    await startSync(config);
    await focusSearch();
  } else if (config.enabled) {
    setupVisible.value = true;
    await nextTick();
    serverInput.value?.focus();
  } else {
    await focusSearch();
  }
});

onBeforeUnmount(() => {
  if (ageRefreshTimer !== undefined) window.clearInterval(ageRefreshTimer);
  if (feedbackTimer !== undefined) window.clearTimeout(feedbackTimer);
  if (feedbackWindowHideTimer !== undefined) window.clearTimeout(feedbackWindowHideTimer);
  document.removeEventListener("keydown", handleKeys);
  unlisteners.forEach((unlisten) => unlisten());
  if (shareReceiverListener) void shareReceiverListener.unregister();
  syncClient?.stop();
  if (globalShortcutApi && !isPasteWindow) void globalShortcutApi.unregister(HOTKEY);
});
</script>

<template>
  <main v-if="isFeedbackWindow" class="feedback-window-shell" aria-hidden="true"></main>

  <main v-else-if="initializing" class="setup-shell setup-loading" :class="{ 'mobile-shell': isMobile }">
    <span class="setup-icon" aria-hidden="true"><LoaderCircle :size="24" class="spin" /></span>
    <strong>ClipRoam</strong>
    <span>正在读取连接配置…</span>
  </main>

  <main v-else-if="setupVisible" class="setup-shell" :class="{ 'mobile-shell': isMobile }">
    <header class="titlebar" @mousedown.left="startWindowDrag">
      <div class="brand">
        <span class="brand-mark"><Clipboard :size="16" /></span>
        <strong>ClipRoam</strong>
      </div>
      <button
        v-if="hasSavedSyncConfig"
        class="icon-button"
        type="button"
        title="返回剪贴板历史"
        aria-label="返回剪贴板历史"
        :disabled="testingConnection"
        @click="closeSetup"
      >
        <X :size="17" />
      </button>
    </header>

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
      <header class="sidebar-brand" @mousedown.left="startWindowDrag">
        <span class="brand-mark"><Clipboard :size="17" /></span>
        <span>
          <strong>ClipRoam</strong>
          <small>剪贴板工作区</small>
        </span>
      </header>

      <nav class="sidebar-nav" aria-label="功能模块">
        <span class="nav-section-label">工作区</span>
        <button class="nav-item active" type="button" aria-current="page">
          <Clipboard :size="17" aria-hidden="true" />
          <span>剪贴板历史</span>
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

    <section class="app-content history-content">
      <header class="titlebar workspace-titlebar" @mousedown.left="startWindowDrag">
        <div v-if="isPasteWindow" class="brand">
          <span class="brand-mark"><Clipboard :size="16" /></span>
          <strong>ClipRoam</strong>
          <span class="shortcut">快速粘贴</span>
        </div>
        <div v-else class="page-title">
          <span>工作区</span>
          <h1>剪贴板历史</h1>
        </div>
        <div class="titlebar-actions">
          <span v-if="isMobile" class="mobile-connection" :class="connectionStatus.tone">{{ connectionStatus.label }}</span>
          <button v-if="isMobile && !isPasteWindow" class="icon-button" type="button" title="设置" aria-label="打开设置" @click="openSettings">
            <Settings2 :size="19" />
          </button>
          <button v-else class="icon-button" type="button" title="关闭" :aria-label="isPasteWindow ? '关闭粘贴窗口' : '关闭主窗口'" @click="hideWindow">
            <X :size="17" />
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
      <div class="filter-row" aria-label="剪贴板类型筛选">
        <button :class="{ active: filter === 'all' }" type="button" @click="filter = 'all'">全部</button>
        <button :class="{ active: filter === 'text' }" type="button" @click="filter = 'text'">文本</button>
        <button :class="{ active: filter === 'files' }" type="button" @click="filter = 'files'">文件</button>
        <button :class="{ active: filter === 'image' }" type="button" @click="filter = 'image'">图片</button>
        <button :class="{ active: filter === 'pending-upload' }" type="button" @click="filter = 'pending-upload'">未上传</button>
        <span class="result-count">{{ filteredEntries.length }} 条</span>
        <button v-if="!isPasteWindow" class="clear-button" type="button" @click="clearHistory">清除未固定</button>
      </div>
      </section>

      <section id="history-content" class="history-list" aria-label="剪贴板历史">
      <div
        v-for="entry in filteredEntries"
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
          v-if="entry.kind === 'image' && !isPasteWindow && thumbnailSource(entry)"
          class="image-thumbnail"
          type="button"
          :aria-label="`预览${entry.content}`"
          :title="`预览${entry.content}`"
          @click.stop="openImagePreview(entry)"
          @dblclick.stop
        ><img :src="thumbnailSource(entry)" :alt="entry.content" loading="lazy" /></button>
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
            <Monitor :size="12" /> {{ deviceName(entry) }}
            <span>·</span>
            <span>{{ formatAge(entry.createdAt) }}</span>
            <span>·</span>
            <span class="sync-status" role="img" :title="syncStatusLabel(entry)" :aria-label="syncStatusLabel(entry)">{{ isEntrySynced(entry) ? "☁️" : "⏳" }}</span>
            <template v-if="fileEntrySummary(entry)">
              <span>·</span>
              <span>{{ fileEntrySummary(entry) }}</span>
            </template>
            <template v-if="uploadStatus(entry)">
              <span>·</span>
              <span class="upload-status" :class="{ uploaded: uploadStatus(entry) === '已上传', uploading: uploadStatus(entry)?.startsWith('上传中') }">{{ uploadStatus(entry) }}</span>
            </template>
          </span>
        </span>
        <span v-if="!isPasteWindow" class="entry-actions">
          <span
            v-if="canSaveEntry(entry)"
            class="item-action"
            role="button"
            tabindex="0"
            :title="saveEntryLabel(entry)"
            :aria-label="saveEntryLabel(entry)"
            @click.stop="saveEntry(entry)"
            @keydown.enter.stop="saveEntry(entry)"
          ><LoaderCircle v-if="savingEntryId === entry.id" :size="15" class="spin" /><Download v-else :size="15" /></span>
          <span
            v-if="canManualUpload(entry)"
            class="item-action"
            role="button"
            tabindex="0"
            :title="uploadingEntryId === entry.id ? '正在上传…' : '上传到服务器（小于 100 MB）'"
            :aria-label="uploadingEntryId === entry.id ? '正在上传' : '上传到服务器'"
            @click.stop="uploadEntry(entry)"
            @keydown.enter.stop="uploadEntry(entry)"
          ><LoaderCircle v-if="uploadingEntryId === entry.id || uploadProgressByEntryId[entry.id]" :size="15" class="spin" /><Upload v-else :size="15" /></span>
          <span
            class="item-action"
            :class="{ active: entry.pinned }"
            role="button"
            tabindex="0"
            :title="entry.pinned ? '取消固定' : '固定'"
            :aria-label="entry.pinned ? '取消固定' : '固定'"
            @click.stop="togglePin(entry)"
            @keydown.enter.stop="togglePin(entry)"
          ><Pin :size="15" /></span>
          <span
            class="item-action danger"
            role="button"
            tabindex="0"
            title="删除"
            aria-label="删除"
            @click.stop="removeEntry(entry)"
            @keydown.enter.stop="removeEntry(entry)"
          ><Trash2 :size="15" /></span>
        </span>
      </div>

      <div v-if="!filteredEntries.length" class="empty-state">
        <Search :size="28" />
        <strong>没有匹配内容</strong>
        <span>{{ isMobile ? "其他设备的内容同步后会显示在这里" : "复制文本后会自动保存到这里" }}</span>
      </div>
      </section>

      <footer class="footer-hint">
      <span v-if="isMobile">点按文本复制，点按文件下载到缓存</span>
      <span v-else-if="isPasteWindow">单击记录立即粘贴</span>
      <span v-else>单击选择，双击复制</span>
      <span v-if="!isMobile"><kbd>↑</kbd><kbd>↓</kbd> 选择</span>
      <span v-if="!isMobile"><kbd>Enter</kbd> {{ isPasteWindow ? "粘贴" : "复制" }}</span>
      <span v-if="!isMobile"><kbd>Esc</kbd> 关闭</span>
      <span v-if="!isPasteWindow" class="privacy"><Check :size="13" /> 本地优先</span>
      </footer>
    </section>

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

            <footer v-if="settingsPage === 'general'" class="settings-actions">
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

    <div v-if="!isPasteWindow && previewImage" class="image-preview-backdrop" @mousedown.self="closeImagePreview">
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
  </main>

  <UpdaterDialog v-if="!isPasteWindow && !isFeedbackWindow" locale="zh-CN" />

  <Transition name="feedback-toast">
    <aside
      v-if="feedbackToast"
      class="feedback-toast-layer"
      :class="[`feedback-${feedbackToast.tone}`, { 'tray-feedback': isFeedbackWindow }]"
      :role="feedbackToast.tone === 'error' ? 'alert' : 'status'"
      :aria-live="feedbackToast.tone === 'error' ? 'assertive' : 'polite'"
      aria-atomic="true"
    >
      <span class="feedback-toast-card">
        <CircleCheck v-if="feedbackToast.tone === 'success'" :size="17" aria-hidden="true" />
        <CircleAlert v-else-if="feedbackToast.tone === 'error'" :size="17" aria-hidden="true" />
        <Info v-else :size="17" aria-hidden="true" />
        <span>{{ feedbackToast.message }}</span>
      </span>
    </aside>
  </Transition>
</template>
