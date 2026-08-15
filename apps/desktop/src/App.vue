<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isRegistered, register, unregister } from "@tauri-apps/plugin-global-shortcut";
import type {
  ClipboardEntry,
  ClipboardKind,
  ClipboardManifestEntry,
  Device,
} from "@cliproam/protocol";
import {
  Check,
  Clipboard,
  Cloud,
  CloudOff,
  Download,
  File,
  FileText,
  FolderOpen,
  Image,
  LoaderCircle,
  Monitor,
  Pin,
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

const HOTKEY = "CommandOrControl+Shift+V";
const CONFIGURED_SERVER_ADDRESS = "127.0.0.1:4810";
const CONFIGURED_SERVER_PROTOCOL = "http";
const DEFAULT_SERVER_ADDRESS = CONFIGURED_SERVER_ADDRESS.includes("://")
  ? new URL(CONFIGURED_SERVER_ADDRESS).host
  : CONFIGURED_SERVER_ADDRESS;
const BROWSER_CONFIG_KEY = "cliproam.syncConfig";
const runningInTauri = "__TAURI_INTERNALS__" in window;
const isPasteWindow = runningInTauri && getCurrentWindow().label === "paste";

type SyncConfig = {
  enabled: boolean;
  serverAddress: string;
  serverProtocol: ServerProtocol;
  username: string;
  sessionToken: string;
  autoUploadLimitMb: number;
};

type MissingFile = { fileId: string; size: number; sourceDeviceId: string };

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
type SettingsPage = "general" | "account" | "data";

const entries = ref<LocalClipboardEntry[]>([]);
const syncedEntryIds = ref(new Set<string>());
const devicesById = ref<Record<string, Device>>({
  browser: { id: "browser", name: "浏览器预览", platform: "browser", osVersion: "未知" },
});
const currentTime = ref(Date.now());
const query = ref("");
const filter = ref<"all" | ClipboardKind | "pending-upload">("all");
const selectedIndex = ref(0);
const connected = ref(false);
const syncEnabled = ref(false);
const errorMessage = ref("");
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
const savingSettings = ref(false);
const changingPassword = ref(false);
const settingsError = ref("");
const passwordChangeError = ref("");
const currentPassword = ref("");
const newPassword = ref("");
const confirmNewPassword = ref("");
const currentUsername = ref("");
const pastingEntryId = ref("");
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

watch(filteredEntries, () => {
  selectedIndex.value = Math.min(selectedIndex.value, Math.max(filteredEntries.value.length - 1, 0));
});

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
  selectedIndex.value = 0;
  await nextTick();
  searchInput.value?.focus();
}

async function hideWindow(): Promise<void> {
  if (runningInTauri) await invoke(isPasteWindow ? "hide_paste" : "hide_main");
}

/**
 * Fetches every content this device is missing. Downloads run through a fixed
 * pool, and one failure no longer aborts the rest — a 3000-file folder should
 * not be lost to a single bad transfer.
 */
async function ensureLocalFiles(entry: LocalClipboardEntry): Promise<LocalClipboardEntry> {
  if (entry.kind !== "files" && entry.kind !== "image") return entry;
  const missing = await invoke<MissingFile[]>("prepare_entry_files", { entryId: entry.id });
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
        mime: undefined,
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

async function paste(entry?: LocalClipboardEntry): Promise<void> {
  if (!entry) return;
  errorMessage.value = "";
  if (!runningInTauri) {
    await navigator.clipboard.writeText(entry.content);
    return;
  }
  if (pastingEntryId.value) return;
  pastingEntryId.value = entry.id;
  try {
    await ensureLocalFiles(entry);
    await invoke("paste_entry", { entryId: entry.id });
  } catch (error) {
    if (String(error).includes("clipboard entry was not found")) {
      await refreshEntries();
      return;
    }
    errorMessage.value = String(error);
  } finally {
    pastingEntryId.value = "";
  }
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
    errorMessage.value = "同步服务未连接，无法上传文件";
    return;
  }
  if (uploadingEntryId.value || uploadProgressByEntryId.value[entry.id] || !canManualUpload(entry)) return;
  uploadingEntryId.value = entry.id;
  errorMessage.value = "";
  try {
    // Publishing needs the tree, which the rendered list does not carry.
    await syncClient.upload(await fullEntry(entry));
  } catch (error) {
    errorMessage.value = `上传失败：${error instanceof Error ? error.message : String(error)}`;
  } finally {
    await refreshEntries();
    uploadingEntryId.value = "";
  }
}

function canSaveEntry(entry: LocalClipboardEntry): boolean {
  return runningInTauri
    && (entry.kind === "files" || entry.kind === "image")
    && entry.summary.contentCount > 0;
}

async function saveEntry(entry: LocalClipboardEntry): Promise<void> {
  if (savingEntryId.value || !canSaveEntry(entry)) return;
  savingEntryId.value = entry.id;
  errorMessage.value = "";
  try {
    const localEntry = await ensureLocalFiles(entry);
    await invoke("save_entry_files", { entryId: localEntry.id });
  } catch (error) {
    errorMessage.value = `另存为失败：${error instanceof Error ? error.message : String(error)}`;
  } finally {
    savingEntryId.value = "";
  }
}

async function openImagePreview(entry: LocalClipboardEntry): Promise<void> {
  if (isPasteWindow || previewLoading.value) return;
  previewLoading.value = true;
  errorMessage.value = "";
  try {
    const localEntry = await ensureLocalFiles(entry);
    if (!imageSource(localEntry)) throw new Error("图片文件不可用");
    previewImage.value = localEntry;
    await nextTick();
    previewDialog.value?.focus();
  } catch (error) {
    errorMessage.value = `无法预览图片：${error instanceof Error ? error.message : String(error)}`;
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
  if (!runningInTauri || event.button !== 0) return;
  const target = event.target as HTMLElement;
  if (target.closest("button, input, [role='button']")) return;
  await getCurrentWindow().startDragging();
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
  settingsPage.value = "general";
  clearPasswordChangeFields();
  settingsError.value = "";
  settingsVisible.value = true;
}

function selectSettingsPage(page: SettingsPage): void {
  settingsPage.value = page;
  settingsError.value = "";
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
  const config = { ...activeSyncConfig, autoUploadLimitMb: autoUploadLimitMb.value };
  try {
    await persistSyncConfig(config);
    activeSyncConfig = config;
    if (config.enabled && config.username && config.sessionToken) await startSync(config);
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
    errorMessage.value = "";
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
    selectedIndex.value = Math.min(selectedIndex.value + 1, filteredEntries.value.length - 1);
  } else if (event.key === "ArrowUp") {
    event.preventDefault();
    selectedIndex.value = Math.max(selectedIndex.value - 1, 0);
  } else if (event.key === "Enter" && !event.shiftKey) {
    event.preventDefault();
    void paste(filteredEntries.value[selectedIndex.value]);
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
      errorMessage.value = `同步历史失败：${error instanceof Error ? error.message : String(error)}`;
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
        if (value) errorMessage.value = "";
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
      onError: (message) => { errorMessage.value = message; },
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
  await refreshEntries();
  ageRefreshTimer = window.setInterval(() => { currentTime.value = Date.now(); }, 10_000);
  document.addEventListener("keydown", handleKeys);

  if (runningInTauri) {
    unlisteners = await Promise.all([
      listen("cliproam://entry-created", refreshEntries),
      // Emitted once every content of an entry has a known id, which for a
      // folder happens after background hashing finishes.
      listen<string>("cliproam://entry-ready", async ({ payload }) => {
        await refreshEntries();
        if (isPasteWindow || !syncClient) return;
        try {
          await syncClient.publish(await invoke<ClipboardEntry>("get_entry", { entryId: payload }));
        } catch (error) {
          errorMessage.value = `自动上传失败：${error instanceof Error ? error.message : String(error)}`;
        }
      }),
      listen("cliproam://history-changed", refreshEntries),
      listen("cliproam://focus-search", focusSearch),
      listen("cliproam://sync-config-changed", () => { void applySavedSyncConfig(); }),
    ]);
    if (!isPasteWindow && !(await isRegistered(HOTKEY))) {
      await register(HOTKEY, (event) => {
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
  document.removeEventListener("keydown", handleKeys);
  unlisteners.forEach((unlisten) => unlisten());
  syncClient?.stop();
  if (runningInTauri && !isPasteWindow) void unregister(HOTKEY);
});
</script>

<template>
  <main v-if="initializing" class="setup-shell setup-loading">
    <span class="setup-icon" aria-hidden="true"><LoaderCircle :size="24" class="spin" /></span>
    <strong>ClipRoam</strong>
    <span>正在读取连接配置…</span>
  </main>

  <main v-else-if="setupVisible" class="setup-shell">
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

  <main v-else class="app-shell" :class="{ 'paste-app': isPasteWindow }">
    <aside v-if="!isPasteWindow" class="sidebar" aria-label="主导航">
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
          <button class="icon-button" type="button" title="关闭" :aria-label="isPasteWindow ? '关闭粘贴窗口' : '关闭主窗口'" @click="hideWindow">
            <X :size="17" />
          </button>
        </div>
      </header>

      <section class="toolbar">
      <label class="search-field">
        <Search :size="17" aria-hidden="true" />
        <input ref="searchInput" v-model="query" type="search" placeholder="搜索剪贴板历史" aria-label="搜索剪贴板历史" />
        <kbd>Enter</kbd>
      </label>
      <div class="filter-row" aria-label="剪贴板类型筛选">
        <button :class="{ active: filter === 'all' }" type="button" @click="filter = 'all'">全部</button>
        <button :class="{ active: filter === 'text' }" type="button" @click="filter = 'text'">文本</button>
        <button :class="{ active: filter === 'files' }" type="button" @click="filter = 'files'">文件</button>
        <button :class="{ active: filter === 'image' }" type="button" @click="filter = 'image'">图片</button>
        <button :class="{ active: filter === 'pending-upload' }" type="button" @click="filter = 'pending-upload'">未上传</button>
        <span class="result-count">{{ filteredEntries.length }} 条</span>
        <button v-if="!isPasteWindow" class="clear-button" type="button" @click="clearHistory">清除未固定</button>
      </div>
      <p v-if="errorMessage" class="error-banner" role="alert">{{ errorMessage }}</p>
      </section>

      <section id="history-content" class="history-list" aria-label="剪贴板历史">
      <div
        v-for="(entry, index) in filteredEntries"
        :key="entry.id"
        class="history-item"
        :class="{ selected: selectedIndex === index, 'image-entry': entry.kind === 'image' }"
        role="button"
        :tabindex="pastingEntryId === entry.id ? -1 : 0"
        :aria-disabled="pastingEntryId === entry.id"
        @mouseenter="selectedIndex = index"
        @dblclick="paste(entry)"
        @click="selectedIndex = index"
        @keydown.enter.stop="paste(entry)"
        @keydown.space.prevent.stop="paste(entry)"
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
          <LoaderCircle v-if="pastingEntryId === entry.id" :size="18" class="spin" />
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
            :title="savingEntryId === entry.id ? '正在另存为…' : '另存为…'"
            :aria-label="savingEntryId === entry.id ? '正在另存为' : '另存为'"
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
        <span>复制文本后会自动保存到这里</span>
      </div>
      </section>

      <footer class="footer-hint">
      <span><kbd>↑</kbd><kbd>↓</kbd> 选择</span>
      <span><kbd>Enter</kbd> 粘贴</span>
      <span><kbd>Esc</kbd> 关闭</span>
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
          </nav>

          <form class="settings-form" @submit.prevent="saveSettings">
            <section v-if="settingsPage === 'general'" id="settings-general-panel" class="settings-page" role="tabpanel" aria-labelledby="general-settings-heading">
              <header class="settings-page-header">
                <h3 id="general-settings-heading">通用</h3>
                <p>配置当前设备的文件同步行为。</p>
              </header>
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

            <section v-else id="settings-data-panel" class="settings-page" role="tabpanel" aria-labelledby="data-page-heading">
              <header class="settings-page-header">
                <h3 id="data-page-heading">应用数据</h3>
                <p>查看当前设备保存的历史和配置文件。</p>
              </header>
              <section class="settings-section" aria-labelledby="data-settings-heading">
                <div class="settings-section-heading">
                  <span class="settings-icon" aria-hidden="true"><FolderOpen :size="18" /></span>
                  <div>
                    <h4 id="data-settings-heading">本地数据目录</h4>
                    <p>包含本地剪贴板历史、同步配置和已保存的文件。</p>
                  </div>
                </div>
                <button class="secondary-button" type="button" :disabled="savingSettings || changingPassword" @click="openAppDataDirectory">打开应用数据</button>
              </section>
            </section>

            <p v-if="settingsError" class="setup-error" role="alert">{{ settingsError }}</p>

            <footer class="settings-actions">
              <button v-if="settingsPage === 'general'" class="primary-button" type="submit" :disabled="savingSettings || changingPassword">
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
</template>
