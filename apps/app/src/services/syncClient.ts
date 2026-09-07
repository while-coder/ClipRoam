import {
  AuthResponseSchema,
  DEFAULT_AUTO_UPLOAD_LIMIT,
  ENTRY_QUERY_BATCH,
  FILE_CHUNK_SIZE,
  EntryActivateResponseSchema,
  EntryManifestResponseSchema,
  DeviceListResponseSchema,
  EntryPublishResponseSchema,
  EntryQueryResponseSchema,
  ServerMessageSchema,
  UploadBeginResponseSchema,
  UploadChunkResponseSchema,
  type AuthResponse,
  type ClientMessage,
  type ClipboardEntry,
  type ClipboardFile,
  type ClipboardManifestEntry,
  type Device,
  type EntryActivateRequest,
  type EntryPublishRequest,
  type EntryQueryRequest,
  type FileRelayRequest,
  type UploadBeginRequest,
  type UploadBeginResponse,
  type UploadChunkResponse,
} from "@cliproam/protocol";
import { invoke } from "@tauri-apps/api/core";

import { mapWithConcurrency, TRANSFER_CONCURRENCY } from "./concurrency";

export const MANUAL_UPLOAD_LIMIT = 100 * 1024 * 1024;

const UPLOAD_BEGIN_TIMEOUT_MS = 30_000;
const UPLOAD_CHUNK_TIMEOUT_MS = 120_000;
const DOWNLOAD_TIMEOUT_MS = 5 * 60_000;
const DOWNLOAD_RETRY_MS = 3_000;
const SERVE_RETRY_BACKOFF_MS = 60_000;
const ENTRY_HTTP_TIMEOUT_MS = 30_000;

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

export async function authenticateAccount(
  address: string,
  username: string,
  password: string,
  mode: AuthMode,
  protocol: ServerProtocol,
  deviceId: string,
): Promise<AuthResponse> {
  const { httpUrl } = getServerUrls(address, protocol);
  let response: Response;
  try {
    response = await fetch(`${httpUrl}/auth/${mode}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username, password, deviceId }),
    });
  } catch {
    throw new Error("无法连接服务器，请检查 IP、端口和网络");
  }

  const body = await response.json().catch(() => undefined) as unknown;
  if (!response.ok) {
    const message = typeof body === "object" && body && "message" in body
      ? String(body.message)
      : `服务器返回错误 ${response.status}`;
    throw new Error(message);
  }
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
  let response: Response;
  try {
    response = await fetch(`${httpUrl}/auth/password`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${sessionToken}`,
      },
      body: JSON.stringify({ currentPassword, newPassword }),
    });
  } catch {
    throw new Error("无法连接服务器，请检查 IP、端口和网络");
  }

  const body = await response.json().catch(() => undefined) as unknown;
  if (!response.ok) {
    const message = typeof body === "object" && body && "message" in body
      ? String(body.message)
      : `服务器返回错误 ${response.status}`;
    throw new Error(message);
  }
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

  async publish(entry: ClipboardEntry): Promise<void> {
    // Publish the metadata first. Other devices can then retrieve the original
    // from this online device while the server copy is still uploading.
    await this.#publishEntry(entry);
    // Only a fresh OS clipboard capture calls publish(). Restores, uploads and
    // metadata edits use separate paths, so they can never overwrite another
    // device's clipboard. File lists stay history-only to avoid eager caches.
    if (entry.kind !== "files") {
      await this.activate(entry.id).catch(() => undefined);
    }
    await this.#uploadEntry(entry, this.autoUploadLimit, false);
  }

  /** Sends an existing entry update (such as pinning) without re-uploading files. */
  async publishMetadata(entry: ClipboardEntry): Promise<void> {
    await this.#publishEntry(entry);
  }

  async restore(entry: ClipboardEntry): Promise<void> {
    // A snapshot can show that the server no longer has this entry. The HTTP
    // response confirms the write directly, so the socket echo-wait becomes a
    // bounded retry around an ordinary publish.
    await this.#publishWithRetry(entry);
    await this.#uploadEntry(entry, this.autoUploadLimit, false, true);
  }

  // A reconcile pass must not spin while the server is unreachable, so each
  // failed attempt backs off like the upload retry loop does.
  async #publishWithRetry(entry: ClipboardEntry): Promise<void> {
    for (let attempt = 0; attempt < 3 && !this.#stopped; attempt++) {
      try {
        await this.#publishEntry(entry);
        return;
      } catch (error) {
        if (!this.#isRecoverableUploadError(error)) throw error;
        await new Promise((resolve) => window.setTimeout(resolve, 2_000));
      }
    }
    if (!this.#stopped) await this.#publishEntry(entry);
  }

  async upload(entry: ClipboardEntry): Promise<void> {
    // A manual upload can be the first time the server learns about this entry.
    // Without the reference, garbage collection would reclaim the contents that
    // were just uploaded.
    await this.#publishEntry(entry);
    await this.#uploadEntry(entry, MANUAL_UPLOAD_LIMIT, true);
  }

  async fetchEntries(entryIds: readonly string[]): Promise<ClipboardEntry[]> {
    const entries: ClipboardEntry[] = [];
    for (let index = 0; index < entryIds.length; index += ENTRY_QUERY_BATCH) {
      entries.push(
        ...await this.#fetchEntryBatch(entryIds.slice(index, index + ENTRY_QUERY_BATCH)),
      );
    }
    return entries;
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
    const files = await invoke<ClipboardFile[]>("list_entry_files", { entryId: entry.id });
    const candidates = files.filter((file) => (
      file.size < sizeLimit && (forceUpload || !file.available)
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

  // The HTTP response is the confirmation the socket echo used to be.
  async #publishEntry(entry: ClipboardEntry): Promise<ClipboardEntry> {
    const request: EntryPublishRequest = {
      deviceId: this.device.id,
      entry: {
        id: entry.id,
        kind: entry.kind,
        content: entry.content,
        html: entry.html,
        rtf: entry.rtf,
        thumbnail: entry.thumbnail,
        tree: entry.tree,
        files: entry.files.map((file) => ({
          fileId: file.fileId,
          size: file.size,
          available: file.available,
        })),
        sourceDeviceId: entry.sourceDeviceId,
        createdAt: entry.createdAt,
        pinned: entry.pinned,
      },
    };
    const response = await this.#httpFetch("POST", "/entries", {
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request),
      signal: AbortSignal.timeout(ENTRY_HTTP_TIMEOUT_MS),
    });
    if (response.status === 401) throw new Error("登录已失效，请重新登录");
    const body = await response.json().catch(() => undefined) as unknown;
    if (!response.ok) throw new Error(this.#uploadErrorMessage(body, response.status));
    const parsed = EntryPublishResponseSchema.safeParse(body);
    if (!parsed.success) throw new Error("服务器返回了不兼容的发布响应");
    return parsed.data.entry;
  }

  // Adds an entry to the live clipboard of every other device. The response
  // also confirms the entry exists, so reconcile can treat 404 as "gone".
  async activate(entryId: string): Promise<ClipboardEntry> {
    const request: EntryActivateRequest = { deviceId: this.device.id };
    const response = await this.#httpFetch(
      "POST",
      `/entries/${encodeURIComponent(entryId)}/activate`,
      {
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(request),
        signal: AbortSignal.timeout(ENTRY_HTTP_TIMEOUT_MS),
      },
    );
    if (response.status === 401) throw new Error("登录已失效，请重新登录");
    const body = await response.json().catch(() => undefined) as unknown;
    if (!response.ok) throw new Error(this.#uploadErrorMessage(body, response.status));
    const parsed = EntryActivateResponseSchema.safeParse(body);
    if (!parsed.success) throw new Error("服务器返回了不兼容的激活响应");
    return parsed.data.entry;
  }

  // Pulls the reconciliation snapshot after the socket's bare `auth.ack`: the
  // full identity manifest (paged, unfiltered) plus the account's devices.
  // Details of missing entries follow through fetchEntries().
  async #fetchConnectionState(): Promise<void> {
    const manifest: ClipboardManifestEntry[] = [];
    for (let page = 1; ; page++) {
      const response = await this.#httpFetch("GET", `/entries/manifest?page=${page}`, {
        signal: AbortSignal.timeout(ENTRY_HTTP_TIMEOUT_MS),
      });
      if (response.status === 401) throw new Error("登录已失效，请重新登录");
      const body = await response.json().catch(() => undefined) as unknown;
      if (!response.ok) throw new Error(this.#uploadErrorMessage(body, response.status));
      const parsed = EntryManifestResponseSchema.safeParse(body);
      if (!parsed.success) throw new Error("服务器返回了不兼容的连接状态响应");
      manifest.push(...parsed.data.manifest);
      if (!parsed.data.hasMore) break;
    }
    this.handlers.onManifest(manifest, await this.#fetchDevices());
  }

  async #fetchDevices(): Promise<Device[]> {
    const response = await this.#httpFetch("GET", "/devices", {
      signal: AbortSignal.timeout(ENTRY_HTTP_TIMEOUT_MS),
    });
    if (response.status === 401) throw new Error("登录已失效，请重新登录");
    const body = await response.json().catch(() => undefined) as unknown;
    if (!response.ok) throw new Error(this.#uploadErrorMessage(body, response.status));
    const parsed = DeviceListResponseSchema.safeParse(body);
    if (!parsed.success) throw new Error("服务器返回了不兼容的设备列表响应");
    return parsed.data.devices;
  }

  async #fetchEntryBatch(entryIds: readonly string[]): Promise<ClipboardEntry[]> {
    const request: EntryQueryRequest = { entryIds: [...entryIds] };
    const response = await this.#httpFetch("POST", "/entries/query", {
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request),
      signal: AbortSignal.timeout(ENTRY_HTTP_TIMEOUT_MS),
    });
    if (response.status === 401) throw new Error("登录已失效，请重新登录");
    const body = await response.json().catch(() => undefined) as unknown;
    if (!response.ok) throw new Error(this.#uploadErrorMessage(body, response.status));
    const parsed = EntryQueryResponseSchema.safeParse(body);
    if (!parsed.success) throw new Error("服务器返回了不兼容的查询响应");
    return parsed.data.entries;
  }

  async downloadFile(entry: ClipboardEntry, file: ClipboardFile): Promise<void> {
    return this.#downloadFileReference(entry.id, file);
  }

  async downloadFileToSave(
    entry: ClipboardEntry,
    file: ClipboardFile,
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
      available: true,
    });
  }

  async #downloadFileReference(
    entryId: string,
    file: ClipboardFile,
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
        reason: error instanceof Error ? error.message : String(error),
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
    file: ClipboardFile,
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
    file: ClipboardFile,
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
      throw new Error(this.#uploadErrorMessage(body, response.status));
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
      throw new Error(this.#uploadErrorMessage(body, response.status));
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
    const message = error instanceof Error ? error.message : String(error);
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
    file: ClipboardFile,
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
    file: ClipboardFile,
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
      let missing = decodeMissing(begin.missing, chunkCount);
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
        const next = decodeMissing(chunk.missing, chunkCount);
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

  async #uploadBegin(file: ClipboardFile): Promise<UploadBeginResponse> {
    const request: UploadBeginRequest = {
      fileId: file.fileId,
      size: file.size,
    };
    const response = await this.#httpFetch("POST", "/upload/begin", {
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request),
      signal: AbortSignal.timeout(UPLOAD_BEGIN_TIMEOUT_MS),
    });
    if (response.status === 401) throw new Error("登录已失效，请重新登录");
    const body = await response.json().catch(() => undefined) as unknown;
    if (!response.ok) throw new Error(this.#uploadErrorMessage(body, response.status));
    const parsed = UploadBeginResponseSchema.safeParse(body);
    if (!parsed.success) throw new Error("服务器返回了不兼容的上传响应");
    return parsed.data;
  }

  // Returns undefined when the server no longer knows this upload — its
  // ledger was swept or discarded; the caller re-begins to pick up the new one.
  async #uploadChunk(fileId: string, index: number, chunk: Uint8Array): Promise<UploadChunkResponse | undefined> {
    const response = await this.#httpFetch("PUT", `/upload/${fileId}?index=${index}`, {
      headers: { "Content-Type": "application/octet-stream" },
      body: chunk,
      signal: AbortSignal.timeout(UPLOAD_CHUNK_TIMEOUT_MS),
    });
    if (response.status === 404) return undefined;
    const body = await response.json().catch(() => undefined) as unknown;
    if (!response.ok) throw new Error(this.#uploadErrorMessage(body, response.status));
    const parsed = UploadChunkResponseSchema.safeParse(body);
    if (!parsed.success) throw new Error("服务器返回了不兼容的上传响应");
    return parsed.data;
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

  #uploadErrorMessage(body: unknown, status: number): string {
    return typeof body === "object" && body && "message" in body
      ? String((body as { message: unknown }).message)
      : `服务器返回错误 ${status}`;
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
          throw new Error(this.#uploadErrorMessage(body, put.status));
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
          this.handlers.onError(`获取同步历史失败：${error instanceof Error ? error.message : String(error)}`);
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
