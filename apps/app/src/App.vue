<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { addPluginListener, invoke, type PluginListener } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { UpdaterDialog } from "@while-coder/tauri-updater-vue";
import { DEFAULT_AUTO_RECEIVE_CLIPBOARD, DEFAULT_AUTO_UPLOAD_LIMIT_MB, DEFAULT_EXCLUDE_PATTERNS, DEFAULT_WATCH_CLIPBOARD } from "./features/sync/syncDefaults";
import {
  Clipboard,
  Cloud,
  CloudOff,
  CloudUpload,
  Download,
  LoaderCircle,
  MonitorSmartphone,
  Upload,
} from "lucide-vue-next";
import { openModalCount } from "@qingfeng346/ui-kit/components/SModal.vue";
import { authenticateAccount, verifyAccountSession } from "./features/sync/syncSetup";
import { startPasteBridge } from "./features/sync/bridge";
import {
  disposeQuickPasteShortcut,
  initializeQuickPasteShortcut,
  quickPasteShortcutStatus,
} from "./features/quick-paste/quickPasteShortcut";
import { showPasteWindow, hideWindow } from "./features/quick-paste/pasteWindow";
import { useUpdater } from "./features/settings/useUpdater";
import { cancelSettingsRequests, initSettings } from "./features/settings/useSettings";
import { settingsVisible } from "./features/settings/useSettings";
import SettingsDialog from "./features/settings/SettingsDialog.vue";
import MobileSettingsDialog from "./features/settings/MobileSettingsDialog.vue";
import HistoryView from "./features/history/HistoryView.vue";
import DownloadsView from "./features/downloads/DownloadsView.vue";
import DevicesView from "./features/devices/DevicesView.vue";
import UploadsView from "./features/uploads/UploadsView.vue";
import PendingUploadsView from "./features/pending-upload/PendingUploadsView.vue";
import SetupWizard from "./features/setup/SetupWizard.vue";
import type { SetupDraft } from "./features/setup/SetupWizard.vue";
import {
  activatingEntryIds,
  activateFromView,
  removeEntry,
  saveEntry,
  savingEntryId,
} from "./features/history/useEntryActions";
import EntryDownloadToasts from "./features/downloads/EntryDownloadToasts.vue";
import ToastLayer from "./features/toast/ToastLayer.vue";
import { disposeToast, showToast, startToastWindowListener } from "./features/toast/useToast";
import { errorMessage } from "./utils/error";
import { getDevice } from "./utils/device";
import { isToastWindow, isPasteWindow, usePlatform } from "./composables/usePlatform";
import { activeView } from "./composables/useActiveView";
import { activeAccountSession } from "./features/sync/accountSession";
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
  fetchHistoryPage,
  focusSearchInput,
  historyRevision,
  initHistorySync,
} from "./features/history/useHistorySync";
import {
  cancelPendingRefresh,
  cancelUploadProgressFlush,
  uploadProgressByEntryId,
  pendingCount,
  pendingEntries,
  refreshPending,
  refreshPendingCount,
  removePendingEntry,
} from "./features/pending-upload/usePendingUploads";
import {
  activeDownloadCount,
  downloadRecords,
  cancelDownloadRecord,
  removeDownloadRecords,
  refreshDownloadTasks,
  stopAllDownloads,
  ensureLocalFiles,
} from "./features/downloads/useDownloads";
import {
  activeUploadCount,
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
const mobilePages = computed(() => [
  { view: "history" as const, label: "历史", icon: Clipboard, count: 0 },
  { view: "pending-upload" as const, label: "待上传", icon: CloudUpload, count: pendingCount.value },
  { view: "uploads" as const, label: "上传", icon: Upload, count: activeUploadCount.value },
  { view: "downloads" as const, label: "下载", icon: Download, count: activeDownloadCount.value },
  { view: "devices" as const, label: "设备", icon: MonitorSmartphone, count: 0 },
]);

const { initUpdaterVersion } = useUpdater();

const historyView = ref<InstanceType<typeof HistoryView>>();
let pasteSessionReloadRevision = 0;

async function reloadPasteSession(): Promise<void> {
  const revision = ++pasteSessionReloadRevision;
  stopSyncClient({ activeOnly: true });
  setActiveConfig(undefined);
  currentUsername.value = "";
  hasSavedSyncConfig.value = false;
  setupVisible.value = false;
  const config = await invoke<SyncConfig | null>("get_authenticated_sync_config");
  if (revision !== pasteSessionReloadRevision) return;
  if (!config?.sessionToken) return;
  const preferences = await loadAccountPreferences();
  if (revision !== pasteSessionReloadRevision) return;
  setActiveConfig(config ?? undefined);
  setActivePreferences(preferences);
  currentUsername.value = config?.username ?? "";
  hasSavedSyncConfig.value = Boolean(config?.sessionToken);
  if (config?.sessionToken) await startSync(config);
}

function reloadPasteSessionFromEvent(): void {
  void reloadPasteSession().catch((error) => showToast(`同步会话读取失败：${errorMessage(error)}`, "error"));
}
const setupWizard = ref<InstanceType<typeof SetupWizard>>();
const currentTime = ref(Date.now());
const initializing = ref(true);
const setupVisible = ref(false);
const setupError = ref("");
const accountRuntimeError = ref("");
const testingConnection = ref(false);
const verifyingSession = ref(false);
const savedSessionConfig = ref<SyncConfig>();
let authenticationController: AbortController | undefined;
const importingShare = ref(false);
let unlisteners: UnlistenFn[] = [];
let ageRefreshTimer: number | undefined;
let shareReceiverListener: PluginListener | undefined;
let shareConsumeRequested = false;

// 历史同步视图（useHistorySync 单例）的装配：引擎客户端经 engine 导出触达，
// 历史视图 ref 留在本组件，经注入读取。
initHistorySync({
  getSyncClient,
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
    hasSavedSyncConfig.value = false;
    savedSessionConfig.value = undefined;
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

function shareImportMessage(summary: ShareImportSummary): string {
  const parts = [
    summary.texts ? `${summary.texts} 条文字` : "",
    summary.images ? `${summary.images} 张图片` : "",
    summary.files ? `${summary.files} 个文件` : "",
  ].filter(Boolean);
  return parts.length ? `已接收${parts.join("、")}` : "";
}

async function consumeMobileShares(): Promise<void> {
  // 未登录只保留原生 inbox，登录成功后再导入并确认消费。
  if (!platformCapabilities.value.shareReceiver || !hasSavedSyncConfig.value || !getActiveConfig()?.sessionToken) return;
  shareConsumeRequested = true;
  if (importingShare.value) return;
  importingShare.value = true;
  try {
    // 导入期间又收到分享时再拉一次，避免 received 事件被并发保护丢掉。
    while (shareConsumeRequested && getActiveConfig()?.sessionToken) {
      shareConsumeRequested = false;
      const summary = await invoke<ShareImportSummary>("consume_mobile_shares");
      if (summary.shares) showToast(shareImportMessage(summary), "success");
    }
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
  if (testingConnection.value) return;
  if (accountRuntimeError.value) { setupError.value = accountRuntimeError.value; return; }
  setupError.value = "";
  const { serverAddress, username, password, serverProtocol } = draft;

  testingConnection.value = true;
  const controller = new AbortController();
  authenticationController = controller;
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
      controller.signal,
    );
    controller.signal.throwIfAborted();
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
    await persistSyncConfig(config);
    setActiveConfig(config);
    currentUsername.value = config.username;
    await invoke("activate_account");
    const accountPreferences = await loadAccountPreferences();
    const preferences: AccountPreferences = {
      // 自动上传档位不能超过服务器单文件上限：登录响应带回，随偏好持久化。
      autoUploadLimitMb: Math.min(
        DEFAULT_AUTO_UPLOAD_LIMIT_MB,
        Math.max(0, session.settings.maxStoredFileMb),
      ),
      autoReceiveClipboard: sameArchive
        ? accountPreferences.autoReceiveClipboard
        : DEFAULT_AUTO_RECEIVE_CLIPBOARD,
      watchClipboard: sameArchive
        ? accountPreferences.watchClipboard
        : DEFAULT_WATCH_CLIPBOARD,
      excludePatterns: sameArchive
        ? accountPreferences.excludePatterns
        : [...DEFAULT_EXCLUDE_PATTERNS],
      serverMaxFileMb: Math.max(1, Math.floor(session.settings.maxStoredFileMb)),
      // 单次复制文件数上限跟随服务器配置，随偏好持久化供 Rust 捕获时读取。
      maxCaptureFileCount: Math.max(1, Math.floor(session.settings.maxCaptureFileCount)),
    };
    await persistAccountPreferences(preferences);
    refreshPending();
    // HTTP 登录成功后准备客户端，设备表和首页查询不等待 WebSocket。
    await startSync(config);
    await enableAccountShortcuts();
    controller.signal.throwIfAborted();
    savedSessionConfig.value = undefined;
    hasSavedSyncConfig.value = true;
    setupVisible.value = false;
    await consumeMobileShares();
    await nextTick();
    await focusSearchInput();
  } catch (error) {
    await stopSyncClient();
    hasSavedSyncConfig.value = false;
    if (controller.signal.aborted) return;
    const message = errorMessage(error);
    if (accountCreated) {
      setupWizard.value?.setAuthMode("login");
      setupError.value = `账号已创建，但同步连接失败：${message}。请重新登录`;
    } else {
      setupError.value = message;
    }
  } finally {
    testingConnection.value = false;
    if (authenticationController === controller) authenticationController = undefined;
  }
}

async function restoreSavedSession(): Promise<void> {
  if (accountRuntimeError.value) { setupError.value = accountRuntimeError.value; return; }
  const config = savedSessionConfig.value;
  if (!config || testingConnection.value) return;
  setupError.value = "";
  testingConnection.value = true;
  verifyingSession.value = true;
  const controller = new AbortController();
  authenticationController = controller;
  try {
    const verified = await verifyAccountSession(config, await getDevice(), controller.signal);
    controller.signal.throwIfAborted();
    await invoke("activate_account");
    const preferences = await loadAccountPreferences();
    await persistAccountPreferences({
      ...preferences,
      autoUploadLimitMb: Math.min(preferences.autoUploadLimitMb, verified.settings.maxStoredFileMb),
      serverMaxFileMb: Math.max(1, Math.floor(verified.settings.maxStoredFileMb)),
      maxCaptureFileCount: Math.max(1, Math.floor(verified.settings.maxCaptureFileCount)),
    });
    await startSync(config);
    await enableAccountShortcuts();
    controller.signal.throwIfAborted();
    savedSessionConfig.value = undefined;
    hasSavedSyncConfig.value = true;
    setupVisible.value = false;
    await consumeMobileShares();
    await nextTick();
    await focusSearchInput();
  } catch (error) {
    await stopSyncClient();
    hasSavedSyncConfig.value = false;
    if (controller.signal.aborted) return;
    const message = errorMessage(error);
    setupError.value = message;
    if (message.startsWith("服务器尚未支持登录状态验证")) showToast(message, "error");
    if (message === "登录已失效，请重新登录" || message === "设备与登录会话不匹配，请重新登录") {
      const expiredConfig = { ...config, sessionToken: "" };
      savedSessionConfig.value = undefined;
      setActiveConfig(expiredConfig);
      await persistSyncConfig(expiredConfig);
      setupWizard.value?.focusPasswordInput();
    }
  } finally {
    testingConnection.value = false;
    verifyingSession.value = false;
    if (authenticationController === controller) authenticationController = undefined;
  }
}

function handleKeys(event: KeyboardEvent): void {
  // SModal（图片预览、嵌套 confirm 等）开着时键盘归模态栈：Esc 只关栈顶弹窗，
  // Tab 在弹窗内循环，不再落到下面触发 hideWindow。
  if (openModalCount.value > 0) return;
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
  if (/deactivate_account.*not found|unknown command.*deactivate_account/i.test(message)) {
    return "原生程序尚未更新，请重新编译并重启 ClipRoam";
  }
  if (message.includes("state not managed for field `state`")) {
    return "应用初始化尚未完成，请从托盘退出 ClipRoam 后重新启动";
  }
  return message || "未知错误";
}

async function initializeTauriServices(): Promise<void> {
  const listenerResults = await Promise.allSettled([
    startToastWindowListener(),
    listen("cliproam://show-paste", () => { void showPasteWindow(); }),
    isPasteWindow
      ? startPasteBridge({ onDevices: rememberDevices, onSessionChanged: reloadPasteSessionFromEvent })
      : Promise.resolve(() => undefined),
    // 兜底：窗口隐藏期间（macOS 对不可见 WKWebView 有节流）可能错过事件，
    // 重新获焦时主动拉一次全量快照，保证「下载」页与行内进度不失真。
    getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (focused) {
        void refreshDownloadTasks().catch(() => undefined);
      }
    }),
  ]);
  unlisteners = listenerResults.flatMap((result) => result.status === "fulfilled" ? [result.value] : []);
  const listenerError = listenerResults.find((result) => result.status === "rejected");
  if (listenerError?.status === "rejected") {
    showToast(`部分后台事件监听初始化失败：${String(listenerError.reason)}`, "error");
  }
  // Read after subscribing so a login during startup cannot be missed.
  if (isPasteWindow) await reloadPasteSession();

  // 窗口打开时先拉一次全量任务快照（此后靠上面的事件跟进）。
  await refreshDownloadTasks().catch(() => undefined);

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

async function enableAccountShortcuts(): Promise<void> {
  if (!isPasteWindow && platformCapabilities.value.globalShortcut) {
    const registered = await initializeQuickPasteShortcut();
    if (!registered) showToast(quickPasteShortcutStatus.value.message, "error");
  }
}

onMounted(async () => {
  if (isToastWindow) {
    unlisteners = [await startToastWindowListener()];
    return;
  }
  // 主窗口重载也必须重新登录，不能继承上一个 WebView 启用的账号服务。
  if (!isPasteWindow) {
    try {
      await withStartupTimeout(invoke("deactivate_account"), "停止旧账号服务超时，请重启 ClipRoam");
    } catch (error) {
      accountRuntimeError.value = readableStartupError(error);
      setupError.value = accountRuntimeError.value;
      console.error("账号服务初始化失败", error);
    }
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
    currentUsername.value = config.username;
    // 保存的 token 必须经服务器验证，才能进入主界面。
    hasSavedSyncConfig.value = false;
    if (config.username && config.sessionToken && !isPasteWindow) savedSessionConfig.value = config;
  }
  if (!isPasteWindow) setupVisible.value = true;
  initializing.value = false;

  if (startupWarning) showToast(startupWarning, "error");
  if (!isPasteWindow) void initUpdaterVersion();
  // The history view fetches its first page itself (revision watch with
  // `immediate`); only the pending badge needs an initial query.
  void refreshPendingCount();
  if (!accountRuntimeError.value) void initializeTauriServices();

  if (setupVisible.value) {
    await nextTick();
    setupWizard.value?.setFields(config ?? undefined);
    if (savedSessionConfig.value && !accountRuntimeError.value) await restoreSavedSession();
    else setupWizard.value?.focusServerInput();
  } else if (config?.username && config.sessionToken) {
    try {
      if (!isPasteWindow) await startSync(config);
    } catch (error) {
      showToast(`同步初始化失败：${errorMessage(error)}`, "error");
    }
    await focusSearchInput();
  }
});

onBeforeUnmount(() => {
  authenticationController?.abort();
  cancelSettingsRequests();
  pasteSessionReloadRevision += 1;
  if (ageRefreshTimer !== undefined) window.clearInterval(ageRefreshTimer);
  disposeToast();
  cancelPendingRefresh();
  cancelUploadProgressFlush();
  document.removeEventListener("keydown", handleKeys);
  unlisteners.forEach((unlisten) => unlisten());
  if (shareReceiverListener) void shareReceiverListener.unregister();
  void stopSyncClient({ activeOnly: isPasteWindow });
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
      :verifying-session="verifyingSession"
      :saved-session="savedSessionConfig"
      @submit="connectAndSave"
      @retry-session="restoreSavedSession"
      @close="closeSetup"
      @reset-error="setupError = ''"
    />
  </main>

  <main v-else :key="activeAccountSession?.id ?? 'signed-out'" class="app-shell" :class="{ 'paste-app': isPasteWindow, 'mobile-app': isMobile }">
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
          :class="{ active: activeView === 'pending-upload' }"
          type="button"
          :aria-current="activeView === 'pending-upload' ? 'page' : undefined"
          @click="activeView = 'pending-upload'"
        >
          <CloudUpload :size="17" aria-hidden="true" />
          <span>待上传</span>
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
        <button
          class="nav-item"
          :class="{ active: activeView === 'devices' }"
          type="button"
          :aria-current="activeView === 'devices' ? 'page' : undefined"
          @click="activeView = 'devices'"
        >
          <MonitorSmartphone :size="17" aria-hidden="true" />
          <span>设备</span>
        </button>
      </nav>

      <div class="sidebar-bottom">
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
      :fetch-history-page="fetchHistoryPage"
      :revision="historyRevision"
      :devices-by-id="devicesById"
      :current-time="currentTime"
      :importing-share="importingShare"
      :activating-entry-ids="activatingEntryIds"
      :saving-entry-id="savingEntryId"
      :ensure-local-files="ensureLocalFiles"
      @activate="activateFromView"
      @remove="removeEntry"
      @save="saveEntry"
    />

    <PendingUploadsView
      v-else-if="activeView === 'pending-upload'"
      :entries="pendingEntries"
      :devices-by-id="devicesById"
      :current-time="currentTime"
      :upload-progress-by-entry-id="uploadProgressByEntryId"
      @remove="removePendingEntry"
    />

    <UploadsView
      v-else-if="activeView === 'uploads'"
      :serve-tasks="uploadTasks"
    />

    <DevicesView
      v-else-if="activeView === 'devices'"
      :current-time="currentTime"
    />

    <DownloadsView
      v-else
      :records="downloadRecords"
      @cancel-record="cancelDownloadRecord($event)"
      @cancel-all="stopAllDownloads()"
      @remove-record="removeDownloadRecords($event)"
      @clear-finished="removeDownloadRecords()"
    />

    <EntryDownloadToasts v-if="isMobile && !isPasteWindow" mobile />

    <nav v-if="isMobile && !isPasteWindow" class="mobile-navigation" aria-label="主导航">
      <button
        v-for="page in mobilePages"
        :key="page.view"
        type="button"
        class="mobile-nav-item"
        :class="{ active: activeView === page.view }"
        :aria-current="activeView === page.view ? 'page' : undefined"
        @click="activeView = page.view"
      >
        <span class="mobile-nav-icon">
          <component :is="page.icon" :size="18" aria-hidden="true" />
          <span v-if="page.count" class="mobile-nav-count" :aria-label="`${page.count} 个任务`">{{ page.count > 99 ? '99+' : page.count }}</span>
        </span>
        <span>{{ page.label }}</span>
      </button>
    </nav>

    <SettingsDialog
      v-if="!isPasteWindow && !isMobile && settingsVisible"
      :current-username="currentUsername"
    />
    <MobileSettingsDialog
      v-if="!isPasteWindow && isMobile && settingsVisible"
      :current-username="currentUsername"
    />

  </main>

  <UpdaterDialog v-if="!isPasteWindow && !isToastWindow" locale="zh-CN" />

  <ToastLayer />
  <EntryDownloadToasts v-if="!isToastWindow && !isMobile" />
  <SMessageHost />
</template>
