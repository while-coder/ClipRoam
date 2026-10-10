import { computed, watch } from "vue";
import { emitTo, listen } from "@tauri-apps/api/event";
import { isPasteWindow } from "../../composables/usePlatform";
import { activeView } from "../../composables/useActiveView";
import { showToast } from "../toast/useToast";
import { getDevice } from "../../utils/device";
import { SYNC_BRIDGE_DEVICES_EVENT, startSyncBridgeService } from "./bridge";
import { getServerUrls } from "./syncSetup";
import { SyncClient } from "./syncClient";
import { AccountSession, accountStateRef, getAccountSession, replaceAccountSession } from "./accountSession";
import { activateRemoteClipboard, bumpLocalClipboardRevision } from "../history/remoteClipboard";
import { getActivePreferences } from "./syncSession";
import { refreshHistory } from "../history/useHistorySync";
import { finishUploadProgress, queueUploadProgress, refreshPending, refreshPendingEntries } from "../pending-upload/usePendingUploads";
import { applyDownloadSnapshot, refreshDownloadTasks, type DownloadTaskSnapshot } from "../downloads/useDownloads";
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
  showToast(value ? "同步已连接" : "同步连接已断开", value ? "success" : "error");
}

export function stopSyncClient(options: { activeOnly?: boolean } = {}): void {
  const session = getAccountSession();
  if (!options.activeOnly && session?.state.connected.value) showToast("同步连接已断开", "error");
  session?.dispose({ stopNativeTasks: !options.activeOnly });
  replaceAccountSession();
}

export const connectionStatus = computed(() => connected.value
  ? { label: "已连接", title: "已连接到同步服务器", tone: "online" }
  : { label: "与服务器断开连接", title: "正在等待同步服务器重新连接", tone: "disconnected" });

function rememberSessionDevices(session: AccountSession, devices: Device[]): void {
  session.state.devicesById.value = {
    ...session.state.devicesById.value,
    ...Object.fromEntries(devices.map((device) => [device.id, device])),
  };
  if (!isPasteWindow) {
    void emitTo("paste", SYNC_BRIDGE_DEVICES_EVENT, { devices, accountKey: session.accountKey }).catch(() => undefined);
  }
}
export function rememberDevices(devices: Device[]): void {
  const session = getAccountSession();
  if (session) rememberSessionDevices(session, devices);
}
export function setSyncAutoUploadLimit(limitMb: number): void {
  getSyncClient()?.pendingUploads.setAutoUploadLimit(limitMb * 1024 * 1024);
}

async function registerAccountEvents(session: AccountSession): Promise<void> {
  const registrations = [
    session.ownAsync(listen("cliproam://entry-created", session.guard(() => {
      bumpLocalClipboardRevision(); refreshPending();
    }))),
    session.ownAsync(listen("cliproam://pending-changed", session.guard(refreshPending))),
    session.ownAsync(listen("cliproam://history-changed", session.guard(refreshHistory))),
    session.ownAsync(listen<DownloadTaskSnapshot[]>("cliproam://download-changed", session.guard(({ payload }) => {
      applyDownloadSnapshot(payload);
    }))),
  ];
  if (!isPasteWindow) registrations.push(session.ownAsync(startSyncBridgeService({
    getDevices: () => Object.values(session.state.devicesById.value),
    accountKey: session.accountKey,
  })));
  const results = await Promise.allSettled(registrations);
  if (session.signal.aborted) return;
  if (results.some((result) => result.status === "rejected")) showToast("部分账号事件监听初始化失败", "error");
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
      onDelete: guard((entryId) => { void session.invoke("remove_server_entry", { entryId }).catch(() => undefined); }),
      onFileAvailable: guard(refreshHistory),
      onUploadProgress: guard(queueUploadProgress),
      onUploadFinished: guard(finishUploadProgress),
      onError: guard((message) => showToast(message, "error")),
      onServeTasksChanged: guard(() => { session.state.uploadTasks.value = [...client.uploads.serveTasksSnapshot()]; }),
      resolveEntryLabel: (entryId) => client.history.getEntry(entryId).then((entry) => entry.content).catch(() => undefined),
      onAuthenticationFailed: guard((message) => {
        stopSyncClient(); engineDeps.onAuthenticationFailed(message, config);
      }),
    }, session.preferences.autoUploadLimitMb * 1024 * 1024, session);
    session.client = client;
    await session.start(() => registerAccountEvents(session));
    session.signal.throwIfAborted();
    refreshHistory(); refreshPending();
    if (!isPasteWindow) { void client.pullDevices(); client.connect(); }
  } catch (error) {
    if (session.signal.aborted) return;
    stopSyncClient();
    throw error;
  }
}
