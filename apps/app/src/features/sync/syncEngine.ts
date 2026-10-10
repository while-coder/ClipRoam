import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { emitTo } from "@tauri-apps/api/event";
import type { ClipboardEntry } from "@cliproam/protocol";
import { isPasteWindow } from "../../composables/usePlatform";
import { showToast } from "../toast/useToast";
import { getDevice } from "../../utils/device";
import { SYNC_BRIDGE_DEVICES_EVENT } from "./bridge";
import { getServerUrls } from "./syncSetup";
import { SyncClient } from "./syncClient";
import { activateRemoteClipboard } from "../history/remoteClipboard";
import {
  getActivePreferences,
} from "./syncSession";
import {
  refreshHistory,
} from "../history/useHistorySync";
import { uploadTasks } from "../uploads/useUploads";
import { finishUploadProgress, queueUploadProgress } from "../pending-upload/uploadProgress";
import { downloader } from "../downloads/useDownloads";
import type { Device, SyncConfig } from "../../types";

/**
 * 同步引擎的模块级单例（从 App.vue 下沉）：SyncClient 生命周期、连接状态、
 * 设备表、远端激活。登录页 UI 态（setupVisible/setupError/setupWizard）留在
 * App.vue，token 失效后的处理经 initSyncEngine 注入回调触达。
 */
export interface SyncEngineDeps {
  onAuthenticationFailed(message: string, config: SyncConfig): void;
}

let engineDeps: SyncEngineDeps;

export function initSyncEngine(deps: SyncEngineDeps): void {
  engineDeps = deps;
}

export const connected = ref(false);
/** 设备列表完全来自服务器（manifest/presence），不做任何本地预设。 */
export const devicesById = ref<Record<string, Device>>({});

let syncClient: SyncClient | undefined;

export function getSyncClient(): SyncClient | undefined {
  return syncClient;
}

/** 连接状态唯一切换点：只在真实变化时更新，并恰好提示一次（断开→成功、成功→断开各一次）。 */
function setConnectionState(value: boolean): void {
  if (connected.value === value) return;
  connected.value = value;
  showToast(value ? "同步已连接" : "同步连接已断开", value ? "success" : "error");
}

/**
 * Tears the sync client down. `activeOnly` 是卸载兜底：只停 socket；断开
 * 提示与下载中止是运行期断开（设置退出、token 失效）的语义。
 */
export function stopSyncClient(options: { activeOnly?: boolean } = {}): void {
  syncClient?.stop();
  syncClient = undefined;
  if (options.activeOnly) return;
  // 凭据失效时中止下载；窗口卸载不影响 Rust 下载任务。
  downloader.stopAll("同步已断开");
  uploadTasks.value = [];
  setConnectionState(false);
}

export const connectionStatus = computed(() =>
  connected.value
    ? { label: "已连接", title: "已连接到同步服务器", tone: "online" }
    : { label: "与服务器断开连接", title: "正在等待同步服务器重新连接", tone: "disconnected" },
);

export function rememberDevices(devices: Device[]): void {
  devicesById.value = {
    ...devicesById.value,
    ...Object.fromEntries(devices.map((device) => [device.id, device])),
  };
  // paste 窗口不持有 sync 客户端，设备名靠主窗口广播补充。
  if (!isPasteWindow) {
    void emitTo("paste", SYNC_BRIDGE_DEVICES_EVENT, { devices }).catch(() => undefined);
  }
}

/** 偏好热更新：自动上传档位是 SyncClient 构造时固化的，运行期经此下发。 */
export function setSyncAutoUploadLimit(limitMb: number): void {
  syncClient?.pendingUploads.setAutoUploadLimit(limitMb * 1024 * 1024);
}

export async function startSync(config: SyncConfig): Promise<void> {
  syncClient?.stop();
  setConnectionState(false);
  const device = await getDevice();
  const { httpUrl, webSocketUrl } = getServerUrls(config.serverAddress, config.serverProtocol);
  let client: SyncClient;
  client = new SyncClient(
    httpUrl,
    webSocketUrl,
    config.sessionToken,
    device,
    {
      onConnected: setConnectionState,
      onDevices: (devices) => { rememberDevices(devices); },
      onDevicePresence: (device) => { rememberDevices([device]); },
      onEntry: () => {
        if (syncClient === client) refreshHistory();
      },
      onPublished: () => {
        if (syncClient === client) refreshHistory();
      },
      onActivation: (entry) => {
        if (syncClient === client) void activateRemoteClipboard(entry);
      },
      onDelete: (entryId) => {
        void invoke("remove_server_entry", { entryId });
      },
      onFileAvailable: () => {
        // Refresh the current page; its details determine the file statuses to query.
        refreshHistory();
      },
      onUploadProgress: queueUploadProgress,
      onUploadFinished: finishUploadProgress,
      onError: (message) => { showToast(message, "error"); },
      onServeTasksChanged: () => {
        if (syncClient !== client) return;
        uploadTasks.value = [...client.uploads.serveTasksSnapshot()];
      },
      resolveEntryLabel: (entryId) =>
        invoke<ClipboardEntry>("get_entry", { entryId })
          .then((entry) => entry.content)
          .catch(() => undefined),
      onAuthenticationFailed: (message) => {
        if (syncClient !== client) return;
        stopSyncClient();
        engineDeps.onAuthenticationFailed(message, config);
      },
    },
    getActivePreferences().autoUploadLimitMb * 1024 * 1024,
  );
  syncClient = client;
  // 会话客户端就绪即刷新首页/当前页，不以 WebSocket 认证作为 HTTP 查询门槛。
  refreshHistory();
  // 快捷粘贴窗口只使用 HTTP 查询历史；不启动 socket、捕获队列或上传循环。
  if (isPasteWindow) {
    return;
  }
  // 设备表和历史查询走 HTTP，推送通道独立建立并自行重连。
  void client.pullDevices();
  client.connect();
}
