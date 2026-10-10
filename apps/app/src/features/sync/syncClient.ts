import { DeviceListResponseSchema, ServerMessageSchema, type ClientMessage, type ClipboardManifestEntry, type Device } from "@cliproam/protocol";
import { DEFAULT_AUTO_UPLOAD_LIMIT } from "./syncDefaults";
import { createSyncRequester, isTransientNetworkError, type SyncRequester } from "./syncHttp";
import { errorMessage } from "../../utils/error";
import type { AccountSession } from "./accountSession";
import { HistoryClient } from "../history/historyClient";
import { PendingUploader } from "../pending-upload/pendingUploader";
import { RelayUploader } from "../uploads/relayUploader";
const ENTRY_HTTP_TIMEOUT_MS = 30_000;
const HEARTBEAT_INTERVAL_MS = 25_000;
type SyncHandlers = {
  onConnected: (connected: boolean) => void;
  /** 登录后拉取的一次性设备表；此后设备增减走 device.presence 推送。 */
  onDevices: (devices: Device[]) => void;
  onDevicePresence: (device: Device) => void;
  onEntry: () => void;
  /** HTTP confirmation only invalidates the list; details come from cache backfill. */
  onPublished: () => void;
  onActivation: (entry: ClipboardManifestEntry) => void;
  onDelete: (entryId: string) => void;
  onFileAvailable: (fileId: string) => void;
  onUploadProgress: (entryId: string, uploadedBytes: number, totalBytes: number) => void;
  onUploadFinished: (entryId: string) => void;
  onError: (message: string) => void;
  onAuthenticationFailed: (message: string) => void;
  /** 中继应答（file.requested）任务台账变化；「上传」页数据源。 */
  onServeTasksChanged?: () => void;
  resolveEntryLabel?: (entryId: string) => Promise<string | undefined>;
};

/** Session wiring and push notifications; business workers live in their systems. */
export class SyncClient {
  #socket?: WebSocket;
  #reconnectTimer?: number;
  #pingTimer?: number;
  #awaitingPong = false;
  #http: SyncRequester;
  readonly history: HistoryClient;
  readonly pendingUploads: PendingUploader;
  readonly uploads: RelayUploader;
  constructor(httpUrl: string, private readonly webSocketUrl: string, private readonly token: string, device: Device, private readonly handlers: SyncHandlers, autoUploadLimit = DEFAULT_AUTO_UPLOAD_LIMIT, readonly session: AccountSession) {
    this.#http = createSyncRequester(httpUrl, token, this.session);
    this.history = new HistoryClient(this.#http, this.session);
    this.pendingUploads = new PendingUploader(this.#http, device, {
      session: this.session,
      onPublished: handlers.onPublished,
      onUploadProgress: handlers.onUploadProgress,
      onUploadFinished: handlers.onUploadFinished,
      onFileAvailable: handlers.onFileAvailable,
      onError: handlers.onError,
    }, autoUploadLimit);
    this.session.own(() => this.stop());
    this.uploads = new RelayUploader(this.#http, {
      session: this.session,
      onServeTasksChanged: handlers.onServeTasksChanged,
      resolveEntryLabel: handlers.resolveEntryLabel,
    });
  }
  connect(): void {
    this.#open();
    this.pendingUploads.start();
  }
  stop(): void {
    if (this.#reconnectTimer) window.clearTimeout(this.#reconnectTimer);
    this.#stopHeartbeat();
    this.#socket?.close();
  }
  // The login-time device table, pulled over HTTP with no socket involved.
  async pullDevices(): Promise<void> {
    try {
      this.handlers.onDevices(await this.#fetchDevices());
    } catch (error) {
      // An expired session must reach the re-login flow, not a toast. A
      // transient network failure stays silent too: the connection-state
      // toast covers it, and the queue's retry pulse rides through.
      const message = errorMessage(error);
      if (message === "登录已失效，请重新登录") this.handlers.onAuthenticationFailed(message);
      else if (!isTransientNetworkError(error)) this.handlers.onError(`获取设备列表失败：${message}`);
    }
  }

  async #fetchDevices(): Promise<Device[]> {
    const devices = await this.#http.request(
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
        void this.uploads.serveRelayRequest(message).catch(() => undefined);
        return;
      case "auth.ack":
        this.handlers.onConnected(true);
        this.#startHeartbeat();
        // A fresh session gives previously skipped rows another chance.
        this.pendingUploads.retrySkipped();
        return;
      case "pong":
        this.#awaitingPong = false;
        return;
      case "clipboard.created":
        this.handlers.onEntry();
        return;
      case "clipboard.activated":
        this.handlers.onActivation({ id: message.entry.id, version: message.entry.version });
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
