import {
  AuthResponseSchema,
  DEFAULT_AUTO_UPLOAD_LIMIT,
  ENTRY_QUERY_BATCH,
  FILE_CHUNK_SIZE,
  EntryActivateResponseSchema,
  EntryManifestResponseSchema,
  DeviceListResponseSchema,
  EntryPublishResponseSchema,
  type EntryPublishInput,
  EntryQueryResponseSchema,
  FileQueryResponseSchema,
  ServerMessageSchema,
  UploadBeginResponseSchema,
  UploadChunkResponseSchema,
  type AuthResponse,
  type ClientMessage,
  type ClipboardEntry,
  type ClipboardManifestEntry,
  type Device,
  type EntryActivateRequest,
  type EntryPublishRequest,
  type EntryQueryRequest,
  type FileQueryRequest,
  type FileRelayRequest,
  type FileStatus,
  type UploadBeginRequest,
  type UploadBeginResponse,
  type UploadChunkResponse,
} from "@cliproam/protocol";
import { invoke } from "@tauri-apps/api/core";

import { mapWithConcurrency, TRANSFER_CONCURRENCY } from "./concurrency";
import { errorMessage } from "../../utils/error";

export const MANUAL_UPLOAD_LIMIT = 100 * 1024 * 1024;

/** Structural stand-in for the protocol's zod schemas, keeping zod out of this file's imports. */
type Schema<T> = { safeParse: (value: unknown) => { success: true; data: T } | { success: false } };

function errorMessageFromBody(body: unknown, status: number): string {
  return typeof body === "object" && body && "message" in body
    ? String((body as { message: unknown }).message)
    : `服务器返回错误 ${status}`;
}

/** One content an entry references, with what this device last knew the server to hold. */
type UploadCandidate = { fileId: string; size: number; uploaded: boolean };

/** The file-shape fields a download or upload transfer needs. */
type FileReference = { fileId: string; size: number };

/** One row of the Rust-side durable capture queue, with the local entry state the publish flow needs. */
type PendingQueueRow = {
  seq: number;
  kind: ClipboardEntry["kind"];
  content: string;
  extra: Partial<Pick<ClipboardEntry, "html" | "rtf" | "fileInfo" | "imageInfo">>;
  createdAt: string;
  localId: string;
  exists: boolean;
  ready: boolean;
};

const UPLOAD_BEGIN_TIMEOUT_MS = 30_000;
const UPLOAD_CHUNK_TIMEOUT_MS = 120_000;
const DOWNLOAD_TIMEOUT_MS = 5 * 60_000;
const DOWNLOAD_RETRY_MS = 3_000;
const SERVE_RETRY_BACKOFF_MS = 60_000;
const ENTRY_HTTP_TIMEOUT_MS = 30_000;
const QUEUE_FAILURE_BACKOFF_MS = 60_000;
const QUEUE_FAILURE_LIMIT = 3;

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

export type AuthMode = "login" | "register";
export type ServerProtocol = "http" | "https";

export function normalizeServerAddress(value: string): string {
  const candidate = value.trim();
  if (!candidate) throw new Error("请输入服务器 IP 和端口");
  if (candidate.includes("://") || candidate.includes("/")) {
    throw new Error("只需填写 IP 和端口，例如 192.168.1.20:4810");
  }

  let url: URL;
  try {
    url = new URL(`http://${candidate}`);
  } catch {
    throw new Error("服务器地址格式不正确");
  }
  if (!url.hostname || !url.port) throw new Error("服务器地址必须包含 IP 和端口");
  return url.host;
}

export function getServerUrls(
  address: string,
  protocol: ServerProtocol,
): { httpUrl: string; webSocketUrl: string } {
  const normalized = normalizeServerAddress(address);
  const secure = protocol === "https";
  return {
    httpUrl: `${secure ? "https" : "http"}://${normalized}`,
    webSocketUrl: `${secure ? "wss" : "ws"}://${normalized}/ws`,
  };
}

async function postJson(httpUrl: string, path: string, body: unknown, headers: Record<string, string> = {}): Promise<unknown> {
  let response: Response;
  try {
    response = await fetch(`${httpUrl}${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...headers },
      body: JSON.stringify(body),
    });
  } catch {
    throw new Error("无法连接服务器，请检查 IP、端口和网络");
  }
  const responseBody = await response.json().catch(() => undefined) as unknown;
  if (!response.ok) throw new Error(errorMessageFromBody(responseBody, response.status));
  return responseBody;
}

export async function authenticateAccount(
  address: string,
  username: string,
  password: string,
  mode: AuthMode,
  protocol: ServerProtocol,
  deviceId: string,
): Promise<AuthResponse> {
  const { httpUrl } = getServerUrls(address, protocol);
  const body = await postJson(httpUrl, `/auth/${mode}`, { username, password, deviceId });
  const result = AuthResponseSchema.safeParse(body);
  if (!result.success) throw new Error("服务器返回了不兼容的登录响应");
  return result.data;
}

export async function changeAccountPassword(
  address: string,
  protocol: ServerProtocol,
  sessionToken: string,
  currentPassword: string,
  newPassword: string,
): Promise<void> {
  const { httpUrl } = getServerUrls(address, protocol);
  await postJson(
    httpUrl,
    "/auth/password",
    { currentPassword, newPassword },
    { Authorization: `Bearer ${sessionToken}` },
  );
}

export async function testSyncConnection(
  url: string,
  token: string,
  device: Device,
  timeoutMs = 6000,
): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    const socket = new WebSocket(url);
    let settled = false;

    const finish = (error?: Error) => {
      if (settled) return;
      settled = true;
      window.clearTimeout(timeout);
      socket.close();
      if (error) reject(error);
      else resolve();
    };

    const timeout = window.setTimeout(
      () => finish(new Error("连接超时，请检查服务器地址和网络")),
      timeoutMs,
    );

    socket.addEventListener("open", () => {
      socket.send(JSON.stringify({ type: "auth", token, device } satisfies ClientMessage));
    });
    socket.addEventListener("message", (event) => {
      try {
        const result = ServerMessageSchema.safeParse(JSON.parse(String(event.data)));
        if (!result.success) {
          finish(new Error("服务器返回了不兼容的响应"));
          return;
        }
        if (result.data.type === "auth.ack") finish();
        else if (result.data.type === "error") finish(new Error(result.data.message));
      } catch {
        finish(new Error("服务器返回了无法解析的数据"));
      }
    });
    socket.addEventListener("error", () => finish(new Error("无法连接服务器，请检查地址和网络")));
    socket.addEventListener("close", () => {
      if (!settled) finish(new Error("服务器在认证完成前断开了连接"));
    });
  });
}

export class SyncClient {
  #socket?: WebSocket;
  #reconnectTimer?: number;
  #stopped = false;
  #connected = false;
  #connectionWaiters = new Set<{
    resolve: () => void;
    reject: (error: Error) => void;
    timer: number;
  }>();
  #downloadAborts = new Set<AbortController>();
  #servingFiles = new Set<string>();
  #failedServes = new Map<string, number>();
  #entryUploads = new Map<string, Promise<void>>();
  #drainRunning = false;
  #drainAgain = false;
  #queueFailures = new Map<number, { at: number; count: number }>();

  constructor(
    private readonly httpUrl: string,
    private readonly webSocketUrl: string,
    private readonly token: string,
    private readonly device: Device,
    private readonly handlers: SyncHandlers,
    private readonly autoUploadLimit = DEFAULT_AUTO_UPLOAD_LIMIT,
  ) {}

  connect(): void {
    this.#stopped = false;
    this.#open();
  }

  stop(): void {
    this.#stopped = true;
    this.#connected = false;
    if (this.#reconnectTimer) window.clearTimeout(this.#reconnectTimer);
    this.#socket?.close();
    // Downloads ride HTTP, so they outlive a socket blip — only an explicit
    // stop ends them.
    for (const abort of this.#downloadAborts) abort.abort();
    this.#downloadAborts.clear();
    for (const waiter of this.#connectionWaiters) {
      window.clearTimeout(waiter.timer);
      waiter.reject(new Error("同步连接已断开"));
    }
    this.#connectionWaiters.clear();
  }

  // Nothing waits on socket echoes anymore: entry writes confirm through
  // their HTTP responses and downloads ride HTTP fetches that carry their own
  // abort controllers.

  // Every write returns the server's stored entry: its id and timestamp are
  // server-assigned, and the caller must adopt it into local state.

  async publish(entry: ClipboardEntry): Promise<ClipboardEntry> {
    // Publish the metadata first. Other devices can then retrieve the original
    // from this online device while the server copy is still uploading.
    const stored = await this.#publishEntry(entry);
    await this.#uploadEntry(entry, this.autoUploadLimit, false);
    return stored;
  }

  async upload(entry: ClipboardEntry): Promise<ClipboardEntry> {
    // A manual upload can be the first time the server learns about this entry.
    // Without the reference, garbage collection would reclaim the contents that
    // were just uploaded.
    const stored = await this.#publishEntry(entry);
    await this.#uploadEntry(entry, MANUAL_UPLOAD_LIMIT, true);
    return stored;
  }

  // The durable capture queue is the single replay mechanism: captures land
  // there with their full payload, and this drain publishes them strictly in
  // insertion order. Concurrent calls collapse into the running pass.
  drainQueue(): void {
    if (this.#stopped) return;
    if (this.#drainRunning) {
      this.#drainAgain = true;
      return;
    }
    this.#drainRunning = true;
    void this.#runDrain()
      .catch((error: unknown) => {
        if (!this.#stopped) {
          this.handlers.onError(`同步剪贴板记录失败：${errorMessage(error)}`);
        }
      })
      .finally(() => {
        this.#drainRunning = false;
        if (this.#drainAgain && !this.#stopped) {
          this.#drainAgain = false;
          this.drainQueue();
        }
      });
  }

  // One pass over the queue, head to tail. The pass ends (without error) at
  // the first row that cannot proceed right now — not-ready payload, a recent
  // failure backoff, a lost connection — and a later trigger restarts it.
  async #runDrain(): Promise<void> {
    while (!this.#stopped) {
      const rows = await invoke<PendingQueueRow[]>("list_pending_entries");
      if (!rows.length) return;
      let blocked = false;
      for (const row of rows) {
        if (this.#stopped) return;
        if (!row.exists) {
          // The entry was already adopted, evicted or deleted, so the publish
          // outcome is decided; only the queue row itself is left to clean up.
          await invoke("acknowledge_pending_entry", { seq: row.seq }).catch(() => undefined);
          continue;
        }
        if (!row.ready) {
          // Strict insertion order: a files entry still hashing blocks the
          // whole tail; `entry-ready` restarts the pass when it resolves.
          blocked = true;
          break;
        }
        const failure = this.#queueFailures.get(row.seq);
        if (failure && Date.now() - failure.at < QUEUE_FAILURE_BACKOFF_MS) {
          blocked = true;
          break;
        }
        try {
          await this.#publishQueueRow(row);
          this.#queueFailures.delete(row.seq);
        } catch (error) {
          if (this.#isRecoverableUploadError(error)) {
            // Bounded wait for the socket, then end the pass — the row keeps
            // its place in line for the next trigger.
            await this.#waitForConnection().catch(() => undefined);
            return;
          }
          const attempts = (failure?.count ?? 0) + 1;
          if (attempts >= QUEUE_FAILURE_LIMIT) {
            // Give up on the row (the entry stays local) instead of blocking
            // the whole queue behind it forever.
            this.#queueFailures.delete(row.seq);
            await invoke("acknowledge_pending_entry", { seq: row.seq }).catch(() => undefined);
            this.handlers.onError(
              `剪贴板记录同步失败，已跳过：${errorMessage(error)}`,
            );
            continue;
          }
          this.#queueFailures.set(row.seq, { at: Date.now(), count: attempts });
          blocked = true;
          break;
        }
      }
      if (blocked) return;
    }
  }

  // Publishes one queue row: metadata first, then contents under the local id,
  // then the server's id is adopted (or the just-created server row deleted if
  // the entry vanished mid-publish), then the broadcast activation.
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
    // The upload commands still address the local entry, which keeps its
    // temporary id until `apply_published_entry` swaps it.
    await this.#uploadEntry({ ...payload, id: row.localId } as ClipboardEntry, this.autoUploadLimit, false);
    const adopted = await invoke<boolean>("apply_published_entry", {
      localEntryId: row.localId,
      entry: stored,
    });
    if (!adopted) {
      // The entry was deleted while the publish was in flight.
      await this.delete(stored.id).catch(() => undefined);
    }
    else if (stored.kind !== "files") {
      await this.activate(stored.id).catch(() => undefined);
    }
    await invoke("acknowledge_pending_entry", { seq: row.seq }).catch(() => undefined);
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

  async #uploadEntry(
    entry: ClipboardEntry,
    sizeLimit: number,
    reportFailures: boolean,
    forceUpload = false,
  ): Promise<void> {
    const existingUpload = this.#entryUploads.get(entry.id);
    if (existingUpload) return existingUpload;

    const upload = this.#uploadFiles(entry, sizeLimit, reportFailures, forceUpload);
    this.#entryUploads.set(entry.id, upload);
    try {
      await upload;
    } finally {
      this.#entryUploads.delete(entry.id);
    }
  }

  /**
   * Content ids are known before publishing, so a finished upload never changes
   * the entry — the server just learns it now holds those bytes.
   */
  async #uploadFiles(
    entry: ClipboardEntry,
    sizeLimit: number,
    reportFailures: boolean,
    forceUpload: boolean,
  ): Promise<void> {
    if (entry.kind !== "files" && entry.kind !== "image") return;
    const files = await invoke<UploadCandidate[]>("list_entry_files", { entryId: entry.id });
    const candidates = files.filter((file) => (
      file.size < sizeLimit && (forceUpload || !file.uploaded)
    ));
    if (!candidates.length) {
      if (reportFailures) throw new Error("没有小于 100 MB 的未上传文件");
      return;
    }

    const totalBytes = candidates.reduce((total, file) => total + file.size, 0);
    const uploadedByFileId = new Map(candidates.map((file) => [file.fileId, 0]));
    this.handlers.onUploadProgress(entry.id, 0, totalBytes);
    try {
      const results = await mapWithConcurrency(
        candidates,
        TRANSFER_CONCURRENCY,
        async (file) => {
          await this.#uploadFile(entry, file, (fileUploadedBytes) => {
            uploadedByFileId.set(file.fileId, fileUploadedBytes);
            const uploadedBytes = [...uploadedByFileId.values()].reduce(
              (total, bytes) => total + bytes,
              0,
            );
            this.handlers.onUploadProgress(entry.id, uploadedBytes, totalBytes);
          });
          return file.fileId;
        },
      );
      const uploaded = results.flatMap((result) => (
        result.status === "fulfilled" ? [result.value] : []
      ));
      if (uploaded.length) {
        await invoke("mark_files_uploaded", { entryId: entry.id, fileIds: uploaded });
      }
      const sourceFailure = results.find((result) => (
        result.status === "rejected"
        && String(result.reason).includes("复制的源文件已删除或移动")
      ));
      if (sourceFailure?.status === "rejected") {
        this.handlers.onError("部分源文件已删除或移动，已保留剪贴板记录和可用文件");
        if (reportFailures) throw sourceFailure.reason;
        return;
      }
      if (reportFailures) {
        const failure = results.find((result) => result.status === "rejected");
        if (failure?.status === "rejected") throw failure.reason;
      }
    } finally {
      this.handlers.onUploadFinished(entry.id);
    }
  }

  // Shared pipeline for the JSON endpoints: one fetch, one 401 check, one
  // error-body extraction and one schema validation each. With `tolerate404`
  // a missing resource resolves to undefined instead of failing.
  async #request<T>(
    method: string,
    path: string,
    init: RequestInit,
    schema: Schema<T> | null,
    incompatible: string,
    tolerate404 = false,
  ): Promise<T | undefined> {
    const response = await this.#httpFetch(method, path, init);
    if (response.status === 401) throw new Error("登录已失效，请重新登录");
    if (tolerate404 && response.status === 404) return undefined;
    const body = await response.json().catch(() => undefined) as unknown;
    if (!response.ok) throw new Error(errorMessageFromBody(body, response.status));
    if (!schema) return undefined;
    const parsed = schema.safeParse(body);
    if (!parsed.success) throw new Error(incompatible);
    return parsed.data;
  }

  #jsonInit(request: unknown, timeoutMs: number): RequestInit {
    return {
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request),
      signal: AbortSignal.timeout(timeoutMs),
    };
  }

  // The HTTP response is the confirmation the socket echo used to be. The
  // queue-row payload carries no id and no createdAt: identity and timestamp
  // belong to the server, which dedupes by content either way.
  async #publishEntry(entry: EntryPublishInput): Promise<ClipboardEntry> {
    const request: EntryPublishRequest = {
      deviceId: this.device.id,
      entry,
    };
    const stored = await this.#request(
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
    const stored = await this.#request(
      "POST",
      `/entries/${encodeURIComponent(entryId)}/activate`,
      this.#jsonInit(request, ENTRY_HTTP_TIMEOUT_MS),
      EntryActivateResponseSchema,
      "服务器返回了不兼容的激活响应",
    );
    return stored!.entry;
  }

  // Pulls the reconciliation snapshot after the socket's bare `auth.ack`: the
  // newest manifest page (ids only, refetched on every connection — never
  // cached) plus the account's devices. Older pages are deliberately not
  // walked: the local history window is smaller than a page of server rows,
  // and details of missing entries follow through fetchEntries().
  async #fetchConnectionState(): Promise<void> {
    const state = await this.#request(
      "GET",
      "/entries/manifest",
      { signal: AbortSignal.timeout(ENTRY_HTTP_TIMEOUT_MS) },
      EntryManifestResponseSchema,
      "服务器返回了不兼容的连接状态响应",
    );
    this.handlers.onManifest(state!.manifest, await this.#fetchDevices());
  }

  async #fetchDevices(): Promise<Device[]> {
    const devices = await this.#request(
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
    const queried = await this.#request(
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
  async fetchFileStatuses(fileIds: readonly string[]): Promise<FileStatus[]> {
    return this.#queryBatched(fileIds, (batch) => this.#fetchFileStatusBatch(batch));
  }

  async #fetchFileStatusBatch(fileIds: readonly string[]): Promise<FileStatus[]> {
    const request: FileQueryRequest = { fileIds: [...fileIds] };
    const queried = await this.#request(
      "POST",
      "/files/query",
      this.#jsonInit(request, ENTRY_HTTP_TIMEOUT_MS),
      FileQueryResponseSchema,
      "服务器返回了不兼容的文件状态响应",
    );
    return queried!.files;
  }

  async downloadFile(entry: ClipboardEntry, file: FileReference): Promise<void> {
    return this.#downloadFileReference(entry.id, file);
  }

  async downloadFileToSave(
    entry: ClipboardEntry,
    file: FileReference,
    saveId: string,
  ): Promise<void> {
    return this.#downloadFileReference(entry.id, file, saveId);
  }

  async downloadVirtualFile(request: {
    entryId: string;
    fileId: string;
    size: number;
    sourceDeviceId: string;
  }): Promise<void> {
    return this.#downloadFileReference(request.entryId, {
      fileId: request.fileId,
      size: request.size,
    });
  }

  async #downloadFileReference(
    entryId: string,
    file: FileReference,
    saveId?: string,
  ): Promise<void> {
    const transferId = crypto.randomUUID();
    await invoke("begin_file_download", {
      transferId,
      fileId: file.fileId,
      expectedSize: file.size,
      saveId,
    });
    const abort = new AbortController();
    this.#downloadAborts.add(abort);
    try {
      await this.#fetchStoredFile(transferId, entryId, file, abort);
    } catch (error) {
      await invoke("cancel_file_download", {
        transferId,
        reason: errorMessage(error),
      }).catch(() => undefined);
      throw error;
    } finally {
      this.#downloadAborts.delete(abort);
    }
  }

  // Downloads pull raw bytes over one HTTP GET: the server streams stored
  // bytes straight off the pool, and content it does not hold parks the
  // request on a live relay pipe that a device holding the bytes fills through
  // `PUT /files/relay/:sessionId`. A pipe that breaks (or its session expires)
  // simply falls back to the next retry — until the deadline.
  async #fetchStoredFile(
    transferId: string,
    entryId: string,
    file: FileReference,
    abort: AbortController,
  ): Promise<void> {
    const deadline = Date.now() + DOWNLOAD_TIMEOUT_MS;
    let offset = 0;
    for (;;) {
      if (offset >= file.size) {
        await invoke("finish_file_download", { transferId });
        return;
      }
      if (Date.now() >= deadline) throw new Error("文件下载超时，没有设备能够提供该文件");
      try {
        offset = await this.#pullOnce(transferId, entryId, file, offset, abort);
      } catch (error) {
        if (abort.signal.aborted) throw error;
        // Broken pipe, expired session, network hiccup: back off and retry.
        await new Promise((resolve) => setTimeout(resolve, DOWNLOAD_RETRY_MS));
      }
    }
  }

  // One pull attempt: the single download GET either streams stored bytes or
  // parks on the relay pipe until a holder fills it. Returns the offset the
  // transfer has advanced to; completion is the caller's check.
  async #pullOnce(
    transferId: string,
    entryId: string,
    file: FileReference,
    offset: number,
    abort: AbortController,
  ): Promise<number> {
    const response = await this.#httpFetch(
      "GET",
      `/files/${entryId}/${file.fileId}`,
      { signal: abort.signal },
    );
    if (response.status === 401) throw new Error("登录已失效，请重新登录");
    // Error bodies are JSON; the success body is the file itself, so it must
    // stay unread until the streaming loop below.
    if (!response.ok) {
      const body = await response.json().catch(() => undefined) as unknown;
      throw new Error(errorMessageFromBody(body, response.status));
    }
    return this.#drainIntoTransfer(transferId, response, offset);
  }

  async #drainIntoTransfer(transferId: string, response: Response, offset: number): Promise<number> {
    const reader = response.body?.getReader();
    if (!reader) throw new Error("服务器未返回文件内容");
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      await invoke("append_file_download", { transferId, data: bytesToBase64(value) });
      offset += value.byteLength;
    }
    return offset;
  }

  // A 404 is not a failure: another device may have deleted the entry first,
  // and the outcome every device converges on is the same.
  async delete(entryId: string): Promise<void> {
    const response = await this.#httpFetch(
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

  #isRecoverableUploadError(error: unknown): boolean {
    const message = errorMessage(error);
    return message === "同步连接已断开" || message === "同步服务未连接";
  }

  #waitForConnection(): Promise<void> {
    if (this.#stopped) return Promise.reject(new Error("同步连接已断开"));
    if (this.#connected) return Promise.resolve();
    return new Promise<void>((resolve, reject) => {
      const waiter = {
        resolve,
        reject,
        timer: window.setTimeout(() => {
          this.#connectionWaiters.delete(waiter);
          reject(new Error("同步服务重连超时"));
        }, 30_000),
      };
      this.#connectionWaiters.add(waiter);
    });
  }

  #markConnected(): void {
    this.#connected = true;
    for (const waiter of this.#connectionWaiters) {
      window.clearTimeout(waiter.timer);
      waiter.resolve();
    }
    this.#connectionWaiters.clear();
  }

  async #uploadFile(
    entry: ClipboardEntry,
    file: FileReference,
    onProgress: (uploadedBytes: number) => void,
  ): Promise<void> {
    while (!this.#stopped) {
      try {
        await this.#uploadContent(entry.id, file, onProgress);
        return;
      } catch (error) {
        if (this.#stopped || !this.#isRecoverableUploadError(error)) throw error;
        await this.#waitForConnection();
        // The socket can stay open while HTTP is briefly unreachable, so back
        // off instead of spinning on an immediately-failing fetch.
        await new Promise((resolve) => window.setTimeout(resolve, 2_000));
      }
    }
    throw new Error("同步连接已断开");
  }

  // Uploads run over HTTP: one POST handshake that either reports the content
  // already stored or hands back the server's chunk ledger, then raw-byte PUTs
  // that each answer with the authoritative ledger. No socket correlation and
  // no ordering constraint — a chunk only needs its index.
  async #uploadContent(
    entryId: string,
    file: FileReference,
    onProgress: (uploadedBytes: number) => void,
  ): Promise<void> {
    // The ledger can be retired under us — the TTL sweep, or another device's
    // begin discarding state it no longer trusts. A PUT answered with 404 is
    // not a failure: re-beginning hands back the current ledger and the
    // upload continues from it.
    for (let restart = 0; ; restart++) {
      const begin = await this.#uploadBegin(file);
      // The server already had these bytes, so the transfer is over before it
      // began — this is what makes copying a folder twice nearly free.
      if (begin.status === "stored") {
        onProgress(file.size);
        return;
      }
      const chunkCount = Math.ceil(file.size / FILE_CHUNK_SIZE);
      if (begin.receivedBytes > file.size) throw new Error("服务器续传进度超出文件大小");
      // The server may already hold chunks from another device's attempt at
      // the same content, so its bitmap is the only source of truth.
      let missing = decodeMissing(begin.missingChunks, chunkCount);
      onProgress(begin.receivedBytes);
      let retired = false;
      while (missing.length > 0) {
        const index = missing[0]!;
        const offset = index * FILE_CHUNK_SIZE;
        const length = Math.min(FILE_CHUNK_SIZE, file.size - offset);
        const data = await invoke<string>("read_file_chunk", {
          entryId, fileId: file.fileId, offset, length,
        });
        if (!data) throw new Error("本机文件内容不可用");
        // A concurrent upload may store the same content mid-transfer; the
        // chunk response then reports `stored` and the remaining bytes are done.
        const chunk = await this.#uploadChunk(file.fileId, index, base64ToBytes(data));
        if (chunk === undefined) {
          retired = true;
          break;
        }
        if (chunk.status === "stored") {
          onProgress(file.size);
          return;
        }
        const next = decodeMissing(chunk.missingChunks, chunkCount);
        // Bits never clear, so the chunk just sent must have left the ledger;
        // refusing to make progress would loop forever.
        if (next.length >= missing.length) throw new Error("服务器上传进度异常");
        missing = next;
        onProgress(chunk.receivedBytes);
      }
      if (!retired) {
        onProgress(file.size);
        return;
      }
      // A bounded number of restarts keeps a server that keeps answering 404
      // from spinning this loop forever.
      if (restart >= 3) throw new Error("服务器上传进度反复失效");
    }
  }

  async #uploadBegin(file: FileReference): Promise<UploadBeginResponse> {
    const request: UploadBeginRequest = {
      fileId: file.fileId,
      size: file.size,
    };
    const begin = await this.#request(
      "POST",
      "/upload/begin",
      this.#jsonInit(request, UPLOAD_BEGIN_TIMEOUT_MS),
      UploadBeginResponseSchema,
      "服务器返回了不兼容的上传响应",
    );
    return begin!;
  }

  // Returns undefined when the server no longer knows this upload — its
  // ledger was swept or discarded; the caller re-begins to pick up the new one.
  async #uploadChunk(fileId: string, index: number, chunk: Uint8Array): Promise<UploadChunkResponse | undefined> {
    return this.#request(
      "PUT",
      `/upload/${fileId}?index=${index}`,
      {
        headers: { "Content-Type": "application/octet-stream" },
        body: chunk,
        signal: AbortSignal.timeout(UPLOAD_CHUNK_TIMEOUT_MS),
      },
      UploadChunkResponseSchema,
      "服务器返回了不兼容的上传响应",
      true,
    );
  }

  // HTTP is shared by uploads and downloads. Network-level failures surface as
  // a disconnected sync so the caller's retry loop can pick the transfer back
  // up once the connection returns.
  async #httpFetch(method: string, path: string, init: RequestInit): Promise<Response> {
    try {
      return await fetch(`${this.httpUrl}${path}`, {
        ...init,
        method,
        headers: {
          Authorization: `Bearer ${this.token}`,
          ...init.headers as Record<string, string>,
        },
      });
    } catch {
      throw new Error("同步连接已断开");
    }
  }

  // This device may hold the content a `file.requested` push is asking for.
  // Serving it is a loop of local reads streamed as chunked PUTs into the
  // requester's parked relay pipe. Devices without the bytes stay quiet; the
  // sender's own failure backoff is keyed by content so a file we cannot
  // provide is not re-probed per session.
  async #serveRelayRequest(request: FileRelayRequest): Promise<void> {
    if (this.#servingFiles.has(request.sessionId)) return;
    const failedAt = this.#failedServes.get(request.fileId);
    if (failedAt !== undefined && Date.now() - failedAt < SERVE_RETRY_BACKOFF_MS) return;
    // Probing the first byte first: a device that cannot actually provide the
    // content stays quiet instead of poisoning the session for another holder.
    if (request.size > 0) {
      const probe = await invoke<string>("read_file_chunk", {
        entryId: request.entryId,
        fileId: request.fileId,
        offset: 0,
        length: 1,
      });
      if (!probe) {
        this.#rememberFailedServe(request.fileId);
        return;
      }
    }
    this.#servingFiles.add(request.sessionId);
    try {
      let offset = 0;
      for (;;) {
        const length = Math.min(FILE_CHUNK_SIZE, request.size - offset);
        const data = await invoke<string>("read_file_chunk", {
          entryId: request.entryId,
          fileId: request.fileId,
          offset,
          length,
        });
        if (!data) throw new Error("本机文件内容不可用");
        const bytes = base64ToBytes(data);
        const last = offset + bytes.byteLength >= request.size;
        const put = await this.#httpFetch(
          "PUT",
          `/files/relay/${request.sessionId}${last ? "?end=1" : ""}`,
          {
            headers: { "Content-Type": "application/octet-stream" },
            body: bytes,
            signal: AbortSignal.timeout(UPLOAD_CHUNK_TIMEOUT_MS),
          },
        );
        // 410: the requester hung up or the session expired — nothing to serve.
        if (put.status === 410 || put.status === 409) return;
        if (put.status === 401) throw new Error("登录已失效，请重新登录");
        if (!put.ok) {
          const body = await put.json().catch(() => undefined) as unknown;
          throw new Error(errorMessageFromBody(body, put.status));
        }
        offset += bytes.byteLength;
        if (last) return;
      }
    } catch {
      this.#rememberFailedServe(request.fileId);
    } finally {
      this.#servingFiles.delete(request.sessionId);
    }
  }

  #rememberFailedServe(fileId: string): void {
    this.#failedServes.set(fileId, Date.now());
    if (this.#failedServes.size > 100) {
      const cutoff = Date.now() - SERVE_RETRY_BACKOFF_MS;
      for (const [id, at] of this.#failedServes) {
        if (at < cutoff) this.#failedServes.delete(id);
      }
    }
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
        void this.#serveRelayRequest(message).catch(() => undefined);
        return;
      case "auth.ack":
        this.#markConnected();
        this.handlers.onConnected(true);
        // The socket only confirms the session; the manifest and device list
        // ride HTTP. A reconnect re-acks and so re-runs this fetch.
        void this.#fetchConnectionState().catch((error: unknown) => {
          this.handlers.onError(`获取同步历史失败：${errorMessage(error)}`);
        });
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
      this.#connected = false;
      this.handlers.onConnected(false);
      if (!this.#stopped) {
        this.#reconnectTimer = window.setTimeout(() => this.#open(), 2500);
      }
    });

    socket.addEventListener("error", () => socket.close());
  }
}

// The Tauri command reads chunks as base64 (its WebSocket-era shape); uploads
// now send raw bytes, so decode before handing them to fetch. The append
// command keeps that base64 signature, so downloads re-encode each read.
function base64ToBytes(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

// `String.fromCharCode` blows the call stack past ~64K arguments, so the bytes
// go in bounded slices.
function bytesToBase64(bytes: Uint8Array): string {
  const sliceSize = 0x8000;
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += sliceSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + sliceSize));
  }
  return btoa(binary);
}

// Mirrors the server's ledger layout: bit `i` of byte `i >> 3`, least
// significant bit first, one bit per chunk where 1 = still missing. Tail bits
// past `chunkCount` are zero and skipped by the loop bound.
function decodeMissing(missing: string, chunkCount: number): number[] {
  const bytes = base64ToBytes(missing);
  const indices: number[] = [];
  for (let index = 0; index < chunkCount; index++) {
    if (bytes[index >> 3]! & (1 << (index & 7))) indices.push(index);
  }
  return indices;
}
