import {
  ENTRY_PAGE_DEFAULT_LIMIT,
  ENTRY_QUERY_BATCH,
  EntryActivateResponseSchema,
  EntryManifestResponseSchema,
  DeviceListResponseSchema,
  EntryPublishResponseSchema,
  type EntryPublishInput,
  EntryQueryResponseSchema,
  FileQueryResponseSchema,
  ServerMessageSchema,
  type ClientMessage,
  type ClipboardEntry,
  type ClipboardManifestEntry,
  type Device,
  type EntryActivateRequest,
  type EntryPublishRequest,
  type EntryQueryRequest,
  type FileQueryRequest,
  type FileStatus,
} from "@cliproam/protocol";
import { invoke } from "@tauri-apps/api/core";

import { DEFAULT_AUTO_UPLOAD_LIMIT } from "./syncDefaults";
import { FileTransfer } from "./fileTransfer";
import { createSyncRequester, errorMessageFromBody, isTransientNetworkError, type SyncRequester } from "./syncHttp";
import { errorMessage } from "../../utils/error";

const ENTRY_HTTP_TIMEOUT_MS = 30_000;
const QUEUE_FAILURE_BACKOFF_MS = 60_000;
const QUEUE_FAILURE_LIMIT = 3;
const HEARTBEAT_INTERVAL_MS = 25_000;
// 队列排空的轮询间隔：捕获到发布的最大延迟，也是空队列的空转成本。
const DRAIN_POLL_INTERVAL_MS = 2_000;

type SyncHandlers = {
  onConnected: (connected: boolean) => void;
  onManifest: (entries: ClipboardManifestEntry[], devices: Device[]) => void;
  onDevicePresence: (device: Device) => void;
  onEntry: (entry: ClipboardEntry) => void;
  onActivation: (entry: ClipboardEntry) => void;
  onDelete: (entryId: string) => void;
  onFileAvailable: (fileId: string) => void;
  onUploadProgress: (entryId: string, uploadedBytes: number, totalBytes: number) => void;
  onUploadFinished: (entryId: string) => void;
  onError: (message: string) => void;
  onAuthenticationFailed: (message: string) => void;
};

/**
 * One publishable row of the Rust-side durable capture queue. The row is
 * self-contained: a files row's placeholder tree is resolved back into the
 * row itself before Peek hands it over, so `extra` is published as-is. The
 * local display id mirrors the Rust-side `temp_entry_id`: `p${seq}`.
 */
type PendingQueueRow = {
  seq: number;
  kind: ClipboardEntry["kind"];
  content: string;
  extra: Partial<Pick<ClipboardEntry, "html" | "rtf" | "fileInfo" | "imageInfo">>;
};

/**
 * The sync orchestrator. All operations ride HTTP (see `syncHttp.ts`, the
 * file pipeline in `fileTransfer.ts`); the socket is a push-only channel:
 * nothing waits on it, and reconnects do not re-pull — pushes missed during
 * a disconnect window wait for the next login.
 */
export class SyncClient {
  #socket?: WebSocket;
  #reconnectTimer?: number;
  #pingTimer?: number;
  #awaitingPong = false;
  #stopped = false;
  #queueFailures = new Map<number, { at: number; count: number }>();
  // Rows that exhausted their retries stay in the queue but are skipped for
  // this session so they cannot block the rows behind them; a reconnect
  // clears the set and gives them another chance.
  #skippedRows = new Set<number>();
  #http: SyncRequester;
  #files: FileTransfer;

  constructor(
    httpUrl: string,
    private readonly webSocketUrl: string,
    private readonly token: string,
    private readonly device: Device,
    private readonly handlers: SyncHandlers,
    private readonly autoUploadLimit = DEFAULT_AUTO_UPLOAD_LIMIT,
    // 登录后拉取对账快照的每页数量；设置页修改随下次 startSync 生效。
    private readonly manifestPageSize = ENTRY_PAGE_DEFAULT_LIMIT,
  ) {
    this.#http = createSyncRequester(httpUrl, token);
    this.#files = new FileTransfer(this.#http, {
      isStopped: () => this.#stopped,
      onUploadProgress: handlers.onUploadProgress,
      onUploadFinished: handlers.onUploadFinished,
      onFileAvailable: handlers.onFileAvailable,
      onError: handlers.onError,
    });
  }

  connect(): void {
    this.#stopped = false;
    this.#open();
    this.#drainLoop();
  }

  stop(): void {
    this.#stopped = true;
    if (this.#reconnectTimer) window.clearTimeout(this.#reconnectTimer);
    this.#stopHeartbeat();
    this.#socket?.close();
    // 下载已移交 Downloader，由调用方（App.vue stopSyncClient）统一 stopAll。
  }

  // The resident drain loop: the durable capture queue is the single replay
  // mechanism — captures land there with their full payload, and this loop
  // publishes them strictly in insertion order on a fixed pulse. A row that
  // cannot proceed right now (recent failure backoff, lost HTTP) waits for a
  // later pulse; a row that failed three times is skipped for this session so
  // it cannot block the rows behind it — reconnecting gives it another chance.
  async #drainLoop(): Promise<void> {
    while (!this.#stopped) {
      await new Promise((resolve) => window.setTimeout(resolve, DRAIN_POLL_INTERVAL_MS));
      if (this.#stopped) return;
      const row = await invoke<PendingQueueRow | null>("peek_pending_entry", {
        skipSeqs: this.#skippedRows.size ? [...this.#skippedRows] : null,
      }).catch(() => null);
      if (!row) continue;
      const failure = this.#queueFailures.get(row.seq);
      if (failure && Date.now() - failure.at < QUEUE_FAILURE_BACKOFF_MS) continue;
      try {
        await this.#publishQueueRow(row);
        this.#queueFailures.delete(row.seq);
      } catch (error) {
        // Transient failures wait out the backoff without counting against
        // the skip limit — HTTP coming back is expected, not the row's fault.
        if (isTransientNetworkError(error)) {
          this.#queueFailures.set(row.seq, { at: Date.now(), count: failure?.count ?? 0 });
          continue;
        }
        const attempts = (failure?.count ?? 0) + 1;
        if (attempts >= QUEUE_FAILURE_LIMIT) {
          // Stop retrying for this session: the row stays in the queue (its
          // payload lives nowhere else) but no longer blocks the rows behind
          // it. Reconnecting clears the skip set and retries it.
          this.#queueFailures.delete(row.seq);
          this.#skippedRows.add(row.seq);
          this.handlers.onError(
            `剪贴板记录同步失败，已暂时跳过：${errorMessage(error)}`,
          );
          continue;
        }
        this.#queueFailures.set(row.seq, { at: Date.now(), count: attempts });
      }
    }
  }

  // Publishes one queue row: the metadata goes first (other devices can start
  // pulling while the contents upload), then the contents — the upload HTTP is
  // content-addressed and needs no entry id — then the broadcast activation,
  // and the row leaves the queue. The new history entry itself arrives through
  // the server's `clipboard.created` echo.
  async #publishQueueRow(row: PendingQueueRow): Promise<void> {
    const payload: EntryPublishInput = {
      kind: row.kind,
      content: row.content,
      html: row.extra.html ?? undefined,
      rtf: row.extra.rtf ?? undefined,
      fileInfo: row.extra.fileInfo ?? undefined,
      imageInfo: row.extra.imageInfo ?? undefined,
      sourceDeviceId: this.device.id,
    };
    const stored = await this.#publishEntry(payload);
    await this.#files.uploadEntry({ ...payload, id: `p${row.seq}` } as ClipboardEntry, this.autoUploadLimit);
    if (stored.kind !== "files") {
      await this.activate(stored.id).catch(() => undefined);
    }
    await invoke("dequeue_pending_entry", { seq: row.seq }).catch(() => undefined);
  }

  // Splits a long id list into fixed-size batches, collecting per-batch results.
  async #queryBatched<T>(ids: readonly string[], run: (batch: string[]) => Promise<T[]>): Promise<T[]> {
    const results: T[] = [];
    for (let index = 0; index < ids.length; index += ENTRY_QUERY_BATCH) {
      results.push(...await run(ids.slice(index, index + ENTRY_QUERY_BATCH)));
    }
    return results;
  }

  async fetchEntries(entryIds: readonly string[]): Promise<ClipboardEntry[]> {
    return this.#queryBatched(entryIds, (batch) => this.#fetchEntryBatch(batch));
  }

  #jsonInit(request: unknown, timeoutMs: number): RequestInit {
    return {
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request),
      signal: AbortSignal.timeout(timeoutMs),
    };
  }

  // Every write returns the server's stored entry: its id and timestamp are
  // server-assigned, and the caller must adopt it into local state.

  // The HTTP response is the confirmation the socket echo used to be. The
  // queue-row payload carries no id and no createdAt: identity and timestamp
  // belong to the server, which dedupes by content either way.
  async #publishEntry(entry: EntryPublishInput): Promise<ClipboardEntry> {
    const request: EntryPublishRequest = {
      deviceId: this.device.id,
      entry,
    };
    const stored = await this.#http.request(
      "POST",
      "/entries",
      this.#jsonInit(request, ENTRY_HTTP_TIMEOUT_MS),
      EntryPublishResponseSchema,
      "服务器返回了不兼容的发布响应",
    );
    return stored!.entry;
  }

  // Adds an entry to the live clipboard of every other device. The response
  // also confirms the entry exists, so reconcile can treat 404 as "gone".
  async activate(entryId: string): Promise<ClipboardEntry> {
    const request: EntryActivateRequest = { deviceId: this.device.id };
    const stored = await this.#http.request(
      "POST",
      `/entries/${encodeURIComponent(entryId)}/activate`,
      this.#jsonInit(request, ENTRY_HTTP_TIMEOUT_MS),
      EntryActivateResponseSchema,
      "服务器返回了不兼容的激活响应",
    );
    return stored!.entry;
  }

  // The login-time reconciliation snapshot, pulled over HTTP with no socket
  // involved: the full unfiltered manifest (ids only, never cached) plus the
  // account's devices. Reconnects do not re-pull — pushes missed during a
  // disconnect window wait for the next login. The protocol's paging contract
  // is "keep walking while the fetched count is below total" — stopping after
  // one page would strand everything past it on a fresh install or rebuilt
  // profile. Details of missing entries follow through fetchEntries().
  async fetchConnectionState(): Promise<void> {
    try {
      const manifest: ClipboardManifestEntry[] = [];
      for (let page = 1; ; page += 1) {
        const state = await this.#http.request(
          "GET",
          `/entries/manifest?page=${page}&pageSize=${this.manifestPageSize}`,
          { signal: AbortSignal.timeout(ENTRY_HTTP_TIMEOUT_MS) },
          EntryManifestResponseSchema,
          "服务器返回了不兼容的连接状态响应",
        );
        manifest.push(...state!.manifest);
        // The empty-page check ends the walk if `total` drifts upward mid-paging.
        if (manifest.length >= state!.total || state!.manifest.length === 0) break;
      }
      this.handlers.onManifest(manifest, await this.#fetchDevices());
    } catch (error) {
      // An expired session must reach the re-login flow, not a toast. A
      // transient network failure stays silent too: the connection-state
      // toast covers it, and the queue's retry pulse rides through.
      const message = errorMessage(error);
      if (message === "登录已失效，请重新登录") this.handlers.onAuthenticationFailed(message);
      else if (!isTransientNetworkError(error)) this.handlers.onError(`获取同步历史失败：${message}`);
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

  async #fetchEntryBatch(entryIds: readonly string[]): Promise<ClipboardEntry[]> {
    const request: EntryQueryRequest = { entryIds: [...entryIds] };
    const queried = await this.#http.request(
      "POST",
      "/entries/query",
      this.#jsonInit(request, ENTRY_HTTP_TIMEOUT_MS),
      EntryQueryResponseSchema,
      "服务器返回了不兼容的查询响应",
    );
    return queried!.entries;
  }

  // Pool availability for a batch of content ids. This replaces the per-entry
  // `missing` list the protocol dropped: the client asks once per upsert batch
  // which contents the server already holds, so locally stored availability
  // marks stay truthful without the server restamping every entry read.
  async fetchFiles(fileIds: readonly string[]): Promise<FileStatus[]> {
    return this.#queryBatched(fileIds, (batch) => this.#fetchFileStatusBatch(batch));
  }

  async #fetchFileStatusBatch(fileIds: readonly string[]): Promise<FileStatus[]> {
    const request: FileQueryRequest = { fileIds: [...fileIds] };
    const queried = await this.#http.request(
      "POST",
      "/files/query",
      this.#jsonInit(request, ENTRY_HTTP_TIMEOUT_MS),
      FileQueryResponseSchema,
      "服务器返回了不兼容的文件状态响应",
    );
    return queried!.files;
  }

  // A 404 is not a failure: another device may have deleted the entry first,
  // and the outcome every device converges on is the same.
  async delete(entryId: string): Promise<void> {
    const response = await this.#http.fetch(
      "DELETE",
      `/entries/${encodeURIComponent(entryId)}`,
      { signal: AbortSignal.timeout(ENTRY_HTTP_TIMEOUT_MS) },
    );
    if (response.status === 401) throw new Error("登录已失效，请重新登录");
    if (response.status === 404) return;
    if (!response.ok) {
      const body = await response.json().catch(() => undefined) as unknown;
      throw new Error(errorMessageFromBody(body, response.status));
    }
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
    if (this.#stopped) return;
    const socket = new WebSocket(this.webSocketUrl);
    this.#socket = socket;

    socket.addEventListener("open", () => {
      this.#send({ type: "auth", token: this.token, device: this.device });
    });

    socket.addEventListener("message", (event) => {
      try {
        void this.#handleMessage(event.data).catch(() => {
          this.handlers.onError("同步服务返回了无法解析的数据");
        });
      } catch {
        this.handlers.onError("同步服务返回了无法解析的数据");
      }
    });

    socket.addEventListener("close", () => {
      if (this.#socket !== socket) return;
      this.#stopHeartbeat();
      this.handlers.onConnected(false);
      if (!this.#stopped) {
        this.#reconnectTimer = window.setTimeout(() => this.#open(), 2500);
      }
    });

    socket.addEventListener("error", () => socket.close());
  }

  async #handleMessage(data: unknown): Promise<void> {
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
        void this.#files.serveRelayRequest(message).catch(() => undefined);
        return;
      case "auth.ack":
        this.handlers.onConnected(true);
        this.#startHeartbeat();
        // A fresh session gives previously skipped rows another chance.
        this.#skippedRows.clear();
        return;
      case "pong":
        this.#awaitingPong = false;
        return;
      case "clipboard.created":
        this.handlers.onEntry(message.entry);
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
