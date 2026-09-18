import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { emitTo } from "@tauri-apps/api/event";
import type { ClipboardEntry } from "@cliproam/protocol";
import { isMobile, isPasteWindow } from "../../composables/usePlatform";
import { showToast } from "../toast/useToast";
import { errorMessage } from "../../utils/error";
import { getDevice } from "../../utils/device";
import { SYNC_BRIDGE_DEVICES_EVENT } from "./bridge";
import { getServerUrls } from "./syncSetup";
import { SyncClient } from "./syncClient";
import {
  getActiveConfig,
  getActivePreferences,
} from "./syncSession";
import {
  applyRemoteUpserts,
  queueRemoteUpsert,
  refreshHistory,
  syncFileStatuses,
  syncedEntryIds,
} from "../clipboard-history/useHistorySync";
import { finishUploadProgress, queueUploadProgress, uploadTasks } from "../uploads/useUploads";
import { downloader, ensurePasteReady } from "../downloads/useDownloads";
import type { Device, LocalClipboardEntry, SyncConfig } from "../../types";

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

let localClipboardRevision = 0;
let remoteActivationRevision = 0;

/** 本机捕获落库计数；远端激活靠它识别「用户刚复制过」的竞态。 */
export function bumpLocalClipboardRevision(): void {
  localClipboardRevision += 1;
}

/** 连接状态唯一切换点：只在真实变化时更新，并恰好提示一次（断开→成功、成功→断开各一次）。 */
function setConnectionState(value: boolean): void {
  if (connected.value === value) return;
  connected.value = value;
  showToast(value ? "同步已连接" : "同步连接已断开", value ? "success" : "error");
  // 重连成功后重查一次"未存储"的文件可用性，兜住离线期间错过的
  // `file.available` 推送（原对账时机已随登录对账移除）。
  if (value) void syncFileStatuses(true);
}

/**
 * Tears the sync client down. `activeOnly` 是卸载兜底：只停 socket；断开
 * 提示与下载中止是运行期断开（设置退出、token 失效）的语义。
 */
export function stopSyncClient(options: { activeOnly?: boolean } = {}): void {
  syncClient?.stop();
  syncClient = undefined;
  if (options.activeOnly) return;
  // 旧 FileTransfer.stop 的语义：同步断开时中止全部下载（凭据已失效）。
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
  syncClient?.setAutoUploadLimit(limitMb * 1024 * 1024);
}

async function activateRemoteClipboard(entry: ClipboardEntry): Promise<void> {
  const config = getActiveConfig();
  if (
    !getActivePreferences().autoReceiveClipboard
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
    if (getActiveConfig() !== config || !getActivePreferences().autoReceiveClipboard) return;
    if (entry.kind === "image") localEntry = await ensurePasteReady(localEntry);

    // A newer remote activation or a real local copy wins while an image is
    // downloading; never replace content the user copied in the meantime.
    if (
      activationRevision !== remoteActivationRevision
      || startingLocalRevision !== localClipboardRevision
      || getActiveConfig() !== config
      || !getActivePreferences().autoReceiveClipboard
    ) return;
    await invoke("activate_remote_entry", { entryId: localEntry.id });
  } catch (error) {
    if (
      activationRevision === remoteActivationRevision
      && getActiveConfig() === config
      && getActivePreferences().autoReceiveClipboard
    ) {
      showToast(`自动接收剪贴板失败：${errorMessage(error)}`, "error");
    }
  }
}

export async function startSync(config: SyncConfig): Promise<void> {
  // The quick-paste window only reads local history; broadcasts from the main
  // window keep it fresh, and a second socket would double every sync task.
  if (isPasteWindow) return;
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
        void invoke("remove_server_entry", { entryId });
      },
      onFileAvailable: () => {
        // Content-addressed push: the server now holds this content. The
        // persisted row is written by the next `/files/query` backfill —
        // this only nudges the refresh burst that triggers it.
        refreshHistory();
      },
      onUploadProgress: queueUploadProgress,
      onUploadFinished: finishUploadProgress,
      onError: (message) => { showToast(message, "error"); },
      onServeTasksChanged: () => {
        if (syncClient !== client) return;
        uploadTasks.value = [...client.serveTasksSnapshot()];
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
  client.connect();
  // 登录即拉取设备表：纯 HTTP，不依赖 socket；auth.ack 只确认连接。
  // 不做历史对账：本地历史靠实时推送增量维护，登录不回拉服务器存量。
  void client.pullDevices();
}
