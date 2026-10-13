import { computed, watch } from "vue";
import { native } from "../../platform/native";
import { showToast } from "../../core/toast";
import { getDevice } from "../../platform/storage";
import { getServerUrls } from "./syncSetup";
import { SyncClient } from "./syncClient";
import { AccountSession, activeView, accountStateRef, getAccountSession, replaceAccountSession } from "./accountSession";
import { activateRemoteClipboard } from "../history/remoteClipboard";
import { getActivePreferences } from "./syncSession";
import { errorMessage } from "../../utils/error";
import type { Device, SyncConfig } from "../../types";

export interface SyncEngineDeps {
  onAuthenticationFailed(message: string, config: SyncConfig): void;
}
let engineDeps: SyncEngineDeps;
export function initSyncEngine(deps: SyncEngineDeps): void { engineDeps = deps; }
export const connected = accountStateRef("connected");
export const devicesById = accountStateRef("devicesById");
export function getSyncClient(): SyncClient | undefined { return getAccountSession()?.client; }

function setConnectionState(session: AccountSession, value: boolean): void {
  if (session.state.connected.value === value) return;
  session.state.connected.value = value;
  if (value) session.history.invalidate();
  showToast(value ? "同步已连接" : "同步连接已断开", value ? "success" : "error");
}

export async function stopSyncClient(): Promise<void> {
  const session = getAccountSession();
  if (session?.state.connected.value) showToast("同步连接已断开", "error");
  session?.dispose();
  replaceAccountSession();
  await native.quickPasteShortcut?.dispose();
}

export const connectionStatus = computed(() => connected.value
  ? { label: "已连接", title: "已连接到同步服务器", tone: "online" }
  : { label: "与服务器断开连接", title: "正在等待同步服务器重新连接", tone: "disconnected" });

function rememberSessionDevices(session: AccountSession, devices: Device[]): void {
  session.state.devicesById.value = {
    ...session.state.devicesById.value,
    ...Object.fromEntries(devices.map((device) => [device.id, device])),
  };
}
export function setSyncAutoUploadLimit(limitMb: number): void {
  getSyncClient()?.pendingUploader.setAutoUploadLimit(limitMb * 1024 * 1024);
}

async function registerAccountEvents(session: AccountSession): Promise<void> {
  await session.capture.start();
  session.scope.run(() => watch(activeView, (view) => {
    if (view === "pending-upload") void session.pendingUploads.refresh().catch(session.guard((error) => showToast(`待上传记录读取失败：${errorMessage(error)}`, "error")));
  }));
  session.interval(() => {
    if (session.state.activeView.value === "downloads") session.downloads.refresh();
  }, 2000);
  session.downloads.refresh();
}

export async function startSync(config: SyncConfig): Promise<void> {
  const session = new AccountSession(config, getActivePreferences());
  replaceAccountSession(session);
  try {
    await session.start(() => session.ready);
    const device = await session.start(() => getDevice());
    const { httpUrl, webSocketUrl } = getServerUrls(config.serverAddress, config.serverProtocol);
    const guard = session.guard.bind(session);
    const client: SyncClient = new SyncClient(httpUrl, webSocketUrl, config.sessionToken, device, {
      onConnected: guard((value) => setConnectionState(session, value)),
      onDevices: guard((devices) => rememberSessionDevices(session, devices)),
      onDevicePresence: guard((device) => rememberSessionDevices(session, [device])),
      onEntry: guard(() => session.history.invalidate()),
      onActivation: guard((entry) => { void activateRemoteClipboard(entry); }),
      onDelete: guard((entryId) => {
        void session.history.removeLocal(entryId).catch(guard(() => showToast("历史缓存清理失败", "error")));
      }),
      onFileAvailable: guard(() => session.history.invalidate()),
      onUploadProgress: guard((entryId, uploaded, total) => session.pendingUploads.queueProgress(entryId, uploaded, total)),
      onUploadFinished: guard((entryId) => session.pendingUploads.finishProgress(entryId)),
      onError: guard((message) => showToast(message, "error")),
      onAuthenticationFailed: guard((message) => {
        void stopSyncClient(); engineDeps.onAuthenticationFailed(message, config);
      }),
    }, session.preferences.autoUploadLimitMb * 1024 * 1024, session);
    session.client = client;
    await session.start(() => registerAccountEvents(session));
    session.signal.throwIfAborted();
    session.history.invalidate();
    void session.pendingUploads.refresh().catch(guard((error) => showToast(`待上传记录读取失败：${errorMessage(error)}`, "error")));
    void client.pullDevices(); client.connect();
  } catch (error) {
    if (session.signal.aborted) return;
    await stopSyncClient();
    throw error;
  }
}
