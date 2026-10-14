import { errorMessage } from "../../utils";
import { computed, ref, watch } from "vue";
import { DeviceListResponseSchema, ServerMessageSchema, type ClientMessage, type ClipboardManifestEntry, type Device } from "@cliproam/protocol";
import { AccountSession, archiveKeyFor, activeView, accountStateRef, getAccountSession, replaceAccountSession } from "../../core/accountSession";
import { AccountDatabase } from "../../platform/database";
import { native } from "../../platform/native";
import { getDevice, readLoginConfig, writeLoginConfig } from "../../platform/storage";
import { DEFAULT_AUTO_UPLOAD_LIMIT, DEFAULT_SERVER_ADDRESS, DEFAULT_SERVER_PROTOCOL, defaultAccountPreferences } from "../../config";
import { showToast } from "../../platform/runtime";
import { getServerUrls } from "./authApi";
import { createSyncRequester, isTransientNetworkError, type SyncRequester } from "./httpClient";
import { activateRemoteClipboard } from "../history/remoteClipboard";
import { PendingUploader } from "../pending-upload/pendingUploader";
import { RelayUploader } from "../uploads/relayUploader";
import type { AccountPreferences, SyncConfig } from "../../types";

export { archiveKeyFor } from "../../core/accountSession";

export interface ClientConnection {
  readonly http: SyncRequester;
  pullDevices(): Promise<boolean>;
  setAutoUploadLimit(limitMb: number): void;
}

export interface ClientSessionManagerDeps {
  onAuthenticationFailed(message: string, config: SyncConfig): void;
}

/** Current account configuration and connection lifecycle. AccountSession owns account resources. */
export class ClientSessionManager {
  readonly hasSavedSyncConfig = ref(false);
  readonly currentUsername = ref("");
  readonly connected = accountStateRef("connected");
  readonly devicesById = accountStateRef("devicesById");
  readonly connectionStatus = computed(() => this.connected.value
    ? { label: "已连接", title: "已连接到同步服务器", tone: "online" }
    : { label: "与服务器断开连接", title: "正在等待同步服务器重新连接", tone: "disconnected" });
  #activeSyncConfig?: SyncConfig;
  #activePreferences: AccountPreferences = defaultAccountPreferences();
  #configurationQueue: Promise<unknown> = Promise.resolve();
  #deps!: ClientSessionManagerDeps;

  getActiveConfig = (): SyncConfig | undefined => {
    return getAccountSession()?.config ?? this.#activeSyncConfig;
  };

  setActiveConfig = (config: SyncConfig | undefined): void => {
    if (getAccountSession()?.config !== config) replaceAccountSession();
    this.#activeSyncConfig = config;
  };

  getActivePreferences = (): AccountPreferences => {
    return getAccountSession()?.preferences ?? this.#activePreferences;
  };

  loadSyncConfig = async (): Promise<SyncConfig | null> => {
    const stored = await readLoginConfig();
    if (!stored) return null;
    return { ...stored, serverAddress:stored.serverAddress || DEFAULT_SERVER_ADDRESS,
      serverProtocol:stored.serverProtocol === "https" ? "https" : DEFAULT_SERVER_PROTOCOL };
  };

  persistSyncConfig = (config: SyncConfig | null): Promise<void> => {
    const work = this.#configurationQueue.then(() => writeLoginConfig(config));
    this.#configurationQueue = work.catch(() => undefined);
    return work;
  };

  loadAccountPreferences = async (): Promise<AccountPreferences> => {
    const config = this.getActiveConfig();
    if (!config?.username) return defaultAccountPreferences();
    const key = archiveKeyFor(config);
    const stored = await new AccountDatabase(key).get<Partial<AccountPreferences>>("meta", "preferences");
    return { ...defaultAccountPreferences(), ...stored };
  };

  persistAccountPreferences = async (preferences: AccountPreferences): Promise<void> => {
    const config = this.getActiveConfig();
    if (!config?.username) throw new Error("同步账号未登录");
    const session = getAccountSession();
    await new AccountDatabase(archiveKeyFor(config)).put("meta", "preferences", preferences);
    if (session) { session.preferences = preferences; await session.capture.updateWatch(); }
    this.#activePreferences = preferences;
  };

  initialize = (deps: ClientSessionManagerDeps): void => { this.#deps = deps; };

  getConnection = (): ClientConnection | undefined => { return getAccountSession()?.client; };

  #setConnectionState = (session: AccountSession, value: boolean): void => {
    if (session.state.connected.value === value) return;
    session.state.connected.value = value;
    if (value) session.history.invalidate();
    showToast(value ? "同步已连接" : "同步连接已断开", value ? "success" : "error");
  };

  stop = async (): Promise<void> => {
    const session = getAccountSession();
    if (session?.state.connected.value) showToast("同步连接已断开", "error");
    session?.dispose();
    replaceAccountSession();
    await native.quickPasteShortcut?.dispose();
  };

  #rememberSessionDevices = (session: AccountSession, devices: Device[]): void => {
    session.state.devicesById.value = {
      ...session.state.devicesById.value,
      ...Object.fromEntries(devices.map((device) => [device.id, device])),
    };
  };

  setAutoUploadLimit = (limitMb: number): void => {
    this.getConnection()?.setAutoUploadLimit(limitMb);
  };

  #registerAccountEvents = async (session: AccountSession): Promise<void> => {
    await session.capture.start();
    session.scope.run(() => watch(activeView, (view) => {
      if (view === "pending-upload") void session.pendingUploads.refresh().catch(session.guard((error) => showToast(`待上传记录读取失败：${errorMessage(error)}`, "error")));
    }));
    session.interval(() => {
      if (session.state.activeView.value === "downloads") session.downloads.refresh();
    }, 2000);
    session.downloads.refresh();
  };

  start = async (config: SyncConfig): Promise<void> => {
    const session = new AccountSession(config, this.getActivePreferences());
    replaceAccountSession(session);
    try {
      await session.start(() => session.ready);
      const device = await session.start(() => getDevice());
      const { httpUrl, webSocketUrl } = getServerUrls(config.serverAddress, config.serverProtocol);
      const guard = session.guard.bind(session);
      const client: SessionConnection = new SessionConnection(httpUrl, webSocketUrl, config.sessionToken, device, {
        onConnected: guard((value) => this.#setConnectionState(session, value)),
        onDevices: guard((devices) => this.#rememberSessionDevices(session, devices)),
        onDevicePresence: guard((device) => this.#rememberSessionDevices(session, [device])),
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
          void this.stop(); this.#deps.onAuthenticationFailed(message, config);
        }),
      }, session.preferences.autoUploadLimitMb * 1024 * 1024, session);
      session.client = client;
      await session.start(() => this.#registerAccountEvents(session));
      session.signal.throwIfAborted();
      session.history.invalidate();
      void session.pendingUploads.refresh().catch(guard((error) => showToast(`待上传记录读取失败：${errorMessage(error)}`, "error")));
      void client.pullDevices(); client.connect();
    } catch (error) {
      if (session.signal.aborted) return;
      await this.stop();
      throw error;
    }
  };
}

export const clientSessionManager = new ClientSessionManager();

/** Private WebSocket and transfer wiring, bound permanently to one account session. */
const ENTRY_HTTP_TIMEOUT_MS = 30_000;
const HEARTBEAT_INTERVAL_MS = 25_000;
type SyncHandlers = {
  onConnected: (connected: boolean) => void;
  /** 登录后拉取的一次性设备表；此后设备增减走 device.presence 推送。 */
  onDevices: (devices: Device[]) => void;
  onDevicePresence: (device: Device) => void;
  onEntry: () => void;
  onActivation: (entry: ClipboardManifestEntry) => void;
  onDelete: (entryId: string) => void;
  onFileAvailable: (fileId: string) => void;
  onUploadProgress: (entryId: string, uploadedBytes: number, totalBytes: number) => void;
  onUploadFinished: (entryId: string) => void;
  onError: (message: string) => void;
  onAuthenticationFailed: (message: string) => void;
};

/** Session wiring and push notifications; business workers live in their systems. */
class SessionConnection implements ClientConnection {
  #socket?: WebSocket;
  #reconnectTimer?: number;
  #pingTimer?: number;
  #awaitingPong = false;
  readonly http: SyncRequester;
  readonly pendingUploader: PendingUploader;
  readonly relayUploader: RelayUploader;
  constructor(httpUrl: string, private readonly webSocketUrl: string, private readonly token: string, device: Device, private readonly handlers: SyncHandlers, autoUploadLimit = DEFAULT_AUTO_UPLOAD_LIMIT, readonly session: AccountSession) {
    this.http = createSyncRequester(httpUrl, token, this.session);
    session.history.connect(this.http);
    this.pendingUploader = new PendingUploader(this.http, device, {
      session: this.session,
      onUploadProgress: handlers.onUploadProgress,
      onUploadFinished: handlers.onUploadFinished,
      onFileAvailable: handlers.onFileAvailable,
      onError: handlers.onError,
    }, autoUploadLimit);
    this.session.own(() => this.stop());
    this.relayUploader = new RelayUploader(this.http, {
      session: this.session,
    });
  }
  setAutoUploadLimit(limitMb: number): void {
    this.pendingUploader.setAutoUploadLimit(limitMb * 1024 * 1024);
  }

  connect(): void {
    this.#open();
    this.pendingUploader.start();
  }
  stop(): void {
    if (this.#reconnectTimer) window.clearTimeout(this.#reconnectTimer);
    this.#stopHeartbeat();
    this.#socket?.close();
  }
  // The login-time device table, pulled over HTTP with no socket involved.
  async pullDevices(): Promise<boolean> {
    try {
      this.handlers.onDevices(await this.#fetchDevices());
      return true;
    } catch (error) {
      // An expired session must reach the re-login flow, not a toast. A
      // transient network failure stays silent too: the connection-state
      // toast covers it, and the queue's retry pulse rides through.
      const message = errorMessage(error);
      if (message === "登录已失效，请重新登录") this.handlers.onAuthenticationFailed(message);
      else if (!isTransientNetworkError(error)) this.handlers.onError(`获取设备列表失败：${message}`);
      return false;
    }
  }

  async #fetchDevices(): Promise<Device[]> {
    const devices = await this.http.request(
      "GET",
      "/devices",
      { signal: AbortSignal.timeout(ENTRY_HTTP_TIMEOUT_MS) },
      DeviceListResponseSchema,
      "服务器返回了不兼容的设备列表响应",
    );
    return devices!.devices;
  }

  #send(message: ClientMessage): boolean {
    if (this.#socket?.readyState === WebSocket.OPEN) {
      this.#socket.send(JSON.stringify(message));
      return true;
    }
    return false;
  }

  // Liveness heartbeat: a dead peer (power loss, NAT table expiry) leaves the
  // TCP socket OPEN with nothing to read, so the status would show "已连接"
  // forever and pushes would silently stop. One missed pong closes the socket
  // and lets the existing reconnect loop run.
  #startHeartbeat(): void {
    this.#stopHeartbeat();
    this.#awaitingPong = false;
    this.#pingTimer = window.setInterval(() => {
      if (this.#awaitingPong) {
        this.#socket?.close();
        return;
      }
      if (this.#send({ type: "ping" })) this.#awaitingPong = true;
    }, HEARTBEAT_INTERVAL_MS);
  }

  #stopHeartbeat(): void {
    if (this.#pingTimer) window.clearInterval(this.#pingTimer);
    this.#pingTimer = undefined;
    this.#awaitingPong = false;
  }

  #open(): void {
    if (this.session.signal.aborted) return;
    const socket = new WebSocket(this.webSocketUrl);
    this.#socket = socket;

    socket.addEventListener("open", () => {
      this.#send({ type: "auth", token: this.token });
    });

    socket.addEventListener("message", (event) => {
      // #handleMessage 是 async：同步段抛错也会变成 rejected promise，
      // 一个 .catch 就能兜住，不需要外层 try。
      void this.#handleMessage(event.data).catch(() => {
        this.handlers.onError("同步服务返回了无法解析的数据");
      });
    });

    socket.addEventListener("close", () => {
      if (this.#socket !== socket) return;
      this.#stopHeartbeat();
      this.handlers.onConnected(false);
      if (!this.session.signal.aborted) {
        this.#reconnectTimer = window.setTimeout(() => this.#open(), 2500);
      }
    });

    socket.addEventListener("error", () => socket.close());
  }

  async #handleMessage(data: unknown): Promise<void> {
    if (this.session.signal.aborted) return;
    const result = ServerMessageSchema.safeParse(JSON.parse(String(data)));
    if (!result.success) return;
    const message = result.data;
    switch (message.type) {
      case "file.available":
        this.handlers.onFileAvailable(message.fileId);
        return;
      case "file.requested":
        // Fire-and-forget: serving streams a whole file and must not block
        // the socket's message pump.
        void this.relayUploader.serveRelayRequest(message).catch(() => undefined);
        return;
      case "auth.ack":
        this.handlers.onConnected(true);
        this.#startHeartbeat();
        return;
      case "pong":
        this.#awaitingPong = false;
        return;
      case "clipboard.created":
        this.handlers.onEntry();
        return;
      case "clipboard.activated":
        this.handlers.onActivation(message.entry);
        return;
      case "clipboard.deleted":
        this.handlers.onDelete(message.entryId);
        return;
      case "device.presence":
        this.handlers.onDevicePresence(message.device);
        return;
      case "error":
        this.handlers.onError(message.message);
        if (message.code === "AUTH_FAILED" || message.code === "AUTH_REQUIRED") {
          this.handlers.onAuthenticationFailed(message.message);
        }
        return;
    }
  }
}
