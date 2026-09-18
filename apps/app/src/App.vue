<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { addPluginListener, invoke, type PluginListener } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { UpdaterDialog } from "@while-coder/tauri-updater-vue";
import { DEFAULT_AUTO_RECEIVE_CLIPBOARD, DEFAULT_AUTO_UPLOAD_LIMIT_MB, DEFAULT_EXCLUDE_PATTERNS } from "./features/sync/syncDefaults";
import {
  Clipboard,
  Cloud,
  CloudOff,
  CloudUpload,
  Download,
  LoaderCircle,
  Settings2,
  Upload,
} from "lucide-vue-next";
import { authenticateAccount } from "./features/sync/syncSetup";
import { startPasteBridge, startSyncBridgeService } from "./features/sync/bridge";
import type { DownloadTaskSnapshot } from "./features/sync/fileTransfer";
import {
  disposeQuickPasteShortcut,
  initializeQuickPasteShortcut,
  quickPasteShortcutStatus,
} from "./features/quick-paste/quickPasteShortcut";
import { showPasteWindow, hideWindow } from "./features/quick-paste/pasteWindow";
import { useUpdater } from "./features/settings/useUpdater";
import { initSettings } from "./features/settings/useSettings";
import { closeSettings, openSettings, settingsVisible } from "./features/settings/useSettings";
import SettingsDialog from "./features/settings/SettingsDialog.vue";
import HistoryView from "./features/clipboard-history/HistoryView.vue";
import DownloadsView from "./features/downloads/DownloadsView.vue";
import UploadsView from "./features/uploads/UploadsView.vue";
import PendingSyncView from "./features/pending-sync/PendingSyncView.vue";
import SetupWizard from "./features/setup/SetupWizard.vue";
import type { SetupDraft } from "./features/setup/SetupWizard.vue";
import {
  activatingEntryIds,
  activateFromView,
  removeEntry,
  saveEntry,
  savingEntryId,
} from "./features/clipboard-history/useEntryActions";
import ToastLayer from "./features/toast/ToastLayer.vue";
import { disposeToast, showToast, startToastWindowListener } from "./features/toast/useToast";
import { errorMessage } from "./utils/error";
import { getDevice } from "./utils/device";
import { isToastWindow, isPasteWindow, usePlatform } from "./composables/usePlatform";
import { activeView } from "./composables/useActiveView";
import {
  archiveKeyFor,
  currentUsername,
  getActiveConfig,
  getActivePreferences,
  hasSavedSyncConfig,
  loadAccountPreferences,
  loadSyncConfig,
  persistAccountPreferences,
  persistSyncConfig,
  setActiveConfig,
  setActivePreferences,
} from "./features/sync/syncSession";
import {
  bumpLocalClipboardRevision,
  connected,
  connectionStatus,
  devicesById,
  getSyncClient,
  initSyncEngine,
  rememberDevices,
  setSyncAutoUploadLimit,
  startSync,
  stopSyncClient,
} from "./features/sync/syncEngine";
import {
  cancelRefreshBurst,
  fetchManifest,
  flushPendingRemoteUpserts,
  focusSearchInput,
  historyRevision,
  initHistorySync,
  refreshHistory,
} from "./features/clipboard-history/useHistorySync";
import {
  pendingCount,
  pendingEntries,
  refreshPendingCount,
  refreshPendingEntries,
} from "./features/pending-sync/usePendingSync";
import {
  activeDownloadCount,
  downloadProgressByEntryId,
  downloadTasks,
  downloader,
  ensureLocalFiles,
} from "./features/downloads/useDownloads";
import {
  activeUploadCount,
  cancelUploadProgressFlush,
  uploadProgressByEntryId,
  uploadTasks,
} from "./features/uploads/useUploads";
import type {
  AccountPreferences,
  PlatformCapabilities,
  ShareImportSummary,
  ShareReceiverEvent,
  SyncConfig,
} from "./types";

const { platformCapabilities, isMobile, setPlatformCapabilities } = usePlatform();

const { initUpdaterVersion } = useUpdater();

const historyView = ref<InstanceType<typeof HistoryView>>();
const setupWizard = ref<InstanceType<typeof SetupWizard>>();
const currentTime = ref(Date.now());
const initializing = ref(true);
const setupVisible = ref(false);
const setupError = ref("");
const testingConnection = ref(false);
const importingShare = ref(false);
let unlisteners: UnlistenFn[] = [];
let ageRefreshTimer: number | undefined;
/** 「下载」页可见时的快照轮询兜底；事件流丢包时列表最多滞后一个周期。 */
let downloadPollTimer: number | undefined;
let shareReceiverListener: PluginListener | undefined;

// 历史同步视图（useHistorySync 单例）的装配：引擎客户端经 engine 导出触达，
// 历史视图 ref 留在本组件，经注入读取。
initHistorySync({
  getSyncClient,
  refreshPendingCount,
  refreshPendingEntries,
  getHistoryView: () => historyView.value,
});

// token 失效后的登录页处理留在本组件（setupVisible/setupError/setupWizard）；
// 引擎只负责 stale 判断与停机，其余经回调交回。
initSyncEngine({
  onAuthenticationFailed: (message, config) => {
    const alreadyRelogging = setupVisible.value && !getActiveConfig()?.sessionToken;
    const expiredConfig = { ...config, sessionToken: "" };
    setActiveConfig(expiredConfig);
    currentUsername.value = expiredConfig.username;
    setupError.value = message;
    setupVisible.value = true;
    if (!alreadyRelogging) {
      void persistSyncConfig(expiredConfig);
      void nextTick(() => setupWizard.value?.setFields(expiredConfig));
    }
  },
});

// 设置弹窗（useSettings 单例）通过 bridge 触达同步引擎；引擎状态已下沉各模块。
initSettings({
  getActiveConfig,
  setActiveConfig,
  getActivePreferences,
  getUsername: () => currentUsername.value,
  setUsername: (name) => { currentUsername.value = name; },
  persistSyncConfig,
  persistAccountPreferences,
  // 偏好热更新：自动上传档位是 SyncClient 构造时固化的，运行期经此下发。
  applyAutoUploadLimit: setSyncAutoUploadLimit,
  disconnect: stopSyncClient,
  markSignedOut: () => {
    hasSavedSyncConfig.value = false;
  },
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
  focusSearchInput,
});

watch(activeView, (view) => {
  if (view === "pending-sync") void refreshPendingEntries();
});

function shareImportMessage(summary: ShareImportSummary): string {
  const parts = [
    summary.texts ? `${summary.texts} 条文字` : "",
    summary.images ? `${summary.images} 张图片` : "",
    summary.files ? `${summary.files} 个文件` : "",
  ].filter(Boolean);
  return parts.length ? `已接收${parts.join("、")}` : "";
}

async function consumeMobileShares(): Promise<void> {
  if (!platformCapabilities.value.shareReceiver || importingShare.value) return;
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

function closeSetup(): void {
  if (testingConnection.value || !hasSavedSyncConfig.value) return;
  setupVisible.value = false;
  setupError.value = "";
  void nextTick(focusSearchInput);
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
      device,
    );
    accountCreated = draft.authMode === "register";
    // 推送通道不设登录门槛：连不上只影响实时推送，登录后的常驻连接会自行
    // 重连并以 toast 报告状态。
    const config: SyncConfig = {
      serverAddress,
      serverProtocol,
      username: session.user.username,
      sessionToken: session.sessionToken,
    };
    // 重登同一账号档案保留它的偏好；换账号从默认开始，避免跨账号污染。
    const sameArchive = archiveKeyFor(getActiveConfig()) === archiveKeyFor(config);
    const preferences: AccountPreferences = {
      // 自动上传档位不能超过服务器单文件上限：登录响应带回，随偏好持久化。
      autoUploadLimitMb: Math.min(
        DEFAULT_AUTO_UPLOAD_LIMIT_MB,
        Math.max(0, session.settings.maxStoredFileMb),
      ),
      autoReceiveClipboard: sameArchive
        ? getActivePreferences().autoReceiveClipboard
        : DEFAULT_AUTO_RECEIVE_CLIPBOARD,
      excludePatterns: sameArchive
        ? getActivePreferences().excludePatterns
        : [...DEFAULT_EXCLUDE_PATTERNS],
      serverMaxFileMb: Math.max(1, Math.floor(session.settings.maxStoredFileMb)),
      // 单次复制文件数上限跟随服务器配置，随偏好持久化供 Rust 捕获时读取。
      maxCaptureFileCount: Math.max(1, Math.floor(session.settings.maxCaptureFileCount)),
    };
    await persistSyncConfig(config);
    setActiveConfig(config);
    currentUsername.value = config.username;
    hasSavedSyncConfig.value = true;
    await persistAccountPreferences(preferences);
    setupVisible.value = false;
    // 先 startSync 再刷新：burst 200ms 后跑文件状态对账时，新 client 必已就位。
    await startSync(config);
    refreshHistory();
    await nextTick();
    await focusSearchInput();
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
  // The history view handles its own dialogs (image preview) plus selection
  // keys; a true return means the key was consumed.
  if (historyView.value?.handleKeydown(event)) return;
  if (event.key === "Escape") {
    event.preventDefault();
    void hideWindow();
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
      bumpLocalClipboardRevision();
      refreshHistory();
    }),
    listen("cliproam://history-changed", refreshHistory),
    listen("cliproam://show-paste", () => { void showPasteWindow(); }),
    isPasteWindow
      ? startPasteBridge({ onDevices: rememberDevices })
      : startSyncBridgeService({ getDevices: () => Object.values(devicesById.value) }),
    // 下载任务快照由 Rust 全局 Downloader 推送（main / paste 共享同一实例）；
    // Windows 虚拟文件的按需拉取也已在 Rust 侧直接入队，前端不再经手。
    listen<DownloadTaskSnapshot[]>("cliproam://download-changed", ({ payload }) => {
      downloader.applySnapshot(payload);
    }),
    // 兜底：窗口隐藏期间（macOS 对不可见 WKWebView 有节流）可能错过事件，
    // 重新获焦时主动拉一次全量快照，保证「下载」页与行内进度不失真。
    getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (focused) void downloader.refresh().catch(() => undefined);
    }),
  ]);
  unlisteners = listenerResults.flatMap((result) => result.status === "fulfilled" ? [result.value] : []);
  const listenerError = listenerResults.find((result) => result.status === "rejected");
  if (listenerError?.status === "rejected") {
    showToast(`部分后台事件监听初始化失败：${String(listenerError.reason)}`, "error");
  }

  // 窗口打开时先拉一次全量任务快照（此后靠上面的事件跟进）。
  await downloader.refresh().catch(() => undefined);
  // 「下载」页可见时的轮询兜底：隐藏窗口期间丢过事件也能在两个周期内追平。
  downloadPollTimer = window.setInterval(() => {
    if (activeView.value === "downloads") void downloader.refresh().catch(() => undefined);
  }, 2000);

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
  const platformPromise = withStartupTimeout(
    invoke<PlatformCapabilities>("get_platform_capabilities"),
    "读取平台能力超时",
  );
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
    setActiveConfig(config);
    setActivePreferences(await loadAccountPreferences());
    currentUsername.value = config.username;
    // 登录态完整才允许从登录页返回主界面；token 过期重登时保留返回入口。
    hasSavedSyncConfig.value = Boolean(config.username && config.sessionToken);
    if (!config.username || !config.sessionToken) setupVisible.value = true;
  }
  initializing.value = false;

  if (startupWarning) showToast(startupWarning, "error");
  if (!isPasteWindow) void initUpdaterVersion();
  // The history view fetches its first page itself (revision watch with
  // `immediate`); only the pending badge needs an initial query.
  void refreshPendingCount();
  void initializeTauriServices();

  if (setupVisible.value) {
    await nextTick();
    setupWizard.value?.setFields(config ?? undefined);
    setupWizard.value?.focusServerInput();
  } else if (config?.username && config.sessionToken) {
    try {
      await startSync(config);
    } catch (error) {
      showToast(`同步初始化失败：${errorMessage(error)}`, "error");
    }
    await focusSearchInput();
  }
});

onBeforeUnmount(() => {
  if (ageRefreshTimer !== undefined) window.clearInterval(ageRefreshTimer);
  if (downloadPollTimer !== undefined) window.clearInterval(downloadPollTimer);
  disposeToast();
  cancelRefreshBurst();
  cancelUploadProgressFlush();
  flushPendingRemoteUpserts();
  document.removeEventListener("keydown", handleKeys);
  unlisteners.forEach((unlisten) => unlisten());
  if (shareReceiverListener) void shareReceiverListener.unregister();
  stopSyncClient({ activeOnly: true });
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
        <button
          class="nav-item"
          :class="{ active: activeView === 'uploads' }"
          type="button"
          :aria-current="activeView === 'uploads' ? 'page' : undefined"
          @click="activeView = 'uploads'"
        >
          <Upload :size="17" aria-hidden="true" />
          <span>上传</span>
          <span v-if="activeUploadCount" class="nav-count">{{ activeUploadCount }}</span>
        </button>
        <button
          class="nav-item"
          :class="{ active: activeView === 'downloads' }"
          type="button"
          :aria-current="activeView === 'downloads' ? 'page' : undefined"
          @click="activeView = 'downloads'"
        >
          <Download :size="17" aria-hidden="true" />
          <span>下载</span>
          <span v-if="activeDownloadCount" class="nav-count">{{ activeDownloadCount }}</span>
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
      :devices-by-id="devicesById"
      :connection-status="connectionStatus"
      :current-time="currentTime"
      :importing-share="importingShare"
      :activating-entry-ids="activatingEntryIds"
      :saving-entry-id="savingEntryId"
      :upload-progress-by-entry-id="uploadProgressByEntryId"
      :download-progress-by-entry-id="downloadProgressByEntryId"
      :ensure-local-files="ensureLocalFiles"
      @activate="activateFromView"
      @remove="removeEntry"
      @save="saveEntry"
      @refresh="refreshHistory"
      @open-settings="openSettings"
    />

    <PendingSyncView
      v-else-if="activeView === 'pending-sync'"
      :entries="pendingEntries"
      :devices-by-id="devicesById"
      :current-time="currentTime"
      :upload-progress-by-entry-id="uploadProgressByEntryId"
      @remove="removeEntry"
    />

    <UploadsView
      v-else-if="activeView === 'uploads'"
      :serve-tasks="uploadTasks"
    />

    <DownloadsView
      v-else
      :download-tasks="downloadTasks"
      @cancel-download="downloader.cancel($event)"
      @cancel-all-downloads="downloader.stopAll()"
    />

    <SettingsDialog
      v-if="!isPasteWindow && settingsVisible"
      :current-username="currentUsername"
    />

  </main>

  <UpdaterDialog v-if="!isPasteWindow && !isToastWindow" locale="zh-CN" />

  <ToastLayer />
</template>
