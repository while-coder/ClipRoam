import { computed, watch } from "vue";
import { native } from "../../platform/native";
import { activeView } from "../../composables/useActiveView";
import { showToast } from "../toast/useToast";
import { getDevice } from "../../utils/device";
import { getServerUrls } from "./syncSetup";
import { SyncClient } from "./syncClient";
import { AccountSession, accountStateRef, getAccountSession, replaceAccountSession } from "./accountSession";
import { activateRemoteClipboard } from "../history/remoteClipboard";
import { getActivePreferences } from "./syncSession";
import { refreshHistory } from "../history/useHistorySync";
import { finishUploadProgress, queueUploadProgress, refreshPending, refreshPendingEntries } from "../pending-upload/usePendingUploads";
import { refreshDownloadTasks } from "../downloads/useDownloads";
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
  if (value) refreshHistory();
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
  getSyncClient()?.pendingUploads.setAutoUploadLimit(limitMb * 1024 * 1024);
}

async function registerAccountEvents(session: AccountSession): Promise<void> {
  await session.capture.start();
  session.scope.run(() => watch(activeView, (view) => {
    if (view === "pending-upload") void refreshPendingEntries();
  }));
  session.interval(() => {
    if (session.state.activeView.value === "downloads") void refreshDownloadTasks().catch(() => undefined);
  }, 2000);
  void refreshDownloadTasks().catch(() => undefined);
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
      onEntry: guard(refreshHistory),
      onPublished: guard(refreshHistory),
      onActivation: guard((entry) => { void activateRemoteClipboard(entry); }),
      onDelete: guard((entryId) => {
        void session.database.delete("entries",entryId).then(guard(refreshHistory)).catch(guard(() => showToast("历史缓存清理失败", "error")));
      }),
      onFileAvailable: guard(refreshHistory),
      onUploadProgress: guard(queueUploadProgress),
      onUploadFinished: guard(finishUploadProgress),
      onError: guard((message) => showToast(message, "error")),
      onServeTasksChanged: guard(() => { session.state.uploadTasks.value = [...client.uploads.serveTasksSnapshot()]; }),
      resolveEntryLabel: (entryId) => client.history.getEntry(entryId).then((entry) => entry.content).catch(() => undefined),
      onAuthenticationFailed: guard((message) => {
        void stopSyncClient(); engineDeps.onAuthenticationFailed(message, config);
      }),
    }, session.preferences.autoUploadLimitMb * 1024 * 1024, session);
    session.client = client;
    await session.start(() => registerAccountEvents(session));
    session.signal.throwIfAborted();
    refreshHistory(); refreshPending();
    void client.pullDevices(); client.connect();
  } catch (error) {
    if (session.signal.aborted) return;
    await stopSyncClient();
    throw error;
  }
}
