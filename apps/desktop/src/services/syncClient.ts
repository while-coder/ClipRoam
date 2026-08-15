import {
  AuthResponseSchema,
  DEFAULT_AUTO_UPLOAD_LIMIT,
  ENTRY_FETCH_BATCH,
  FILE_CHUNK_SIZE,
  ServerMessageSchema,
  type AuthResponse,
  type ClientMessage,
  type ClipboardEntry,
  type ClipboardFile,
  type ClipboardManifestEntry,
  type Device,
} from "@cliproam/protocol";
import { invoke } from "@tauri-apps/api/core";

import { mapWithConcurrency, TRANSFER_CONCURRENCY } from "./concurrency";

export const MANUAL_UPLOAD_LIMIT = 100 * 1024 * 1024;

/** `stored` means the server already holds the content, so nothing is sent. */
type UploadReady = { stored: true } | { stored: false; offset: number };

type SyncHandlers = {
  onConnected: (connected: boolean) => void;
  onManifest: (entries: ClipboardManifestEntry[], devices: Device[]) => void;
  onDevicePresence: (device: Device) => void;
  onEntry: (entry: ClipboardEntry) => void;
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
  #pending = new Map<string, {
    resolve: () => void;
    reject: (error: Error) => void;
    timer: number;
    writeChain?: Promise<void>;
    entryId?: string;
    file?: ClipboardFile;
  }>();
  #uploadReady = new Map<string, {
    resolve: (ready: UploadReady) => void;
    reject: (error: Error) => void;
    timer: number;
  }>();
  #publishConfirmations = new Map<string, {
    resolve: () => void;
    reject: (error: Error) => void;
    timer: number;
  }>();
  #entryFetches = new Map<string, {
    resolve: (entries: ClipboardEntry[]) => void;
    reject: (error: Error) => void;
    timer: number;
  }>();
  #entryUploads = new Map<string, Promise<void>>();

  constructor(
    private readonly url: string,
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
    this.#rejectActiveTransfers("同步连接已断开");
    for (const waiter of this.#connectionWaiters) {
      window.clearTimeout(waiter.timer);
      waiter.reject(new Error("同步连接已断开"));
    }
    this.#connectionWaiters.clear();
  }

  #rejectActiveTransfers(message: string): void {
    for (const transfer of this.#pending.values()) {
      window.clearTimeout(transfer.timer);
      transfer.reject(new Error(message));
    }
    this.#pending.clear();
    for (const transfer of this.#uploadReady.values()) {
      window.clearTimeout(transfer.timer);
      transfer.reject(new Error(message));
    }
    this.#uploadReady.clear();
    for (const confirmation of this.#publishConfirmations.values()) {
      window.clearTimeout(confirmation.timer);
      confirmation.reject(new Error(message));
    }
    this.#publishConfirmations.clear();
    for (const fetch of this.#entryFetches.values()) {
      window.clearTimeout(fetch.timer);
      fetch.reject(new Error(message));
    }
    this.#entryFetches.clear();
  }

  async publish(entry: ClipboardEntry): Promise<void> {
    // Publish the metadata first. Other devices can then retrieve the original
    // from this online device while the server copy is still uploading.
    this.#publishEntry(entry);
    await this.#uploadEntry(entry, this.autoUploadLimit, false);
  }

  /** Sends an existing entry update (such as pinning) without re-uploading files. */
  publishMetadata(entry: ClipboardEntry): boolean {
    return this.#publishEntry(entry);
  }

  async restore(entry: ClipboardEntry): Promise<void> {
    // A snapshot can show that the server no longer has this entry. Do not
    // treat a WebSocket send as success: retain the local entry until the
    // server echoes it back, then re-upload whatever it is missing.
    await this.#publishAndConfirm(entry);
    await this.#uploadEntry(entry, this.autoUploadLimit, false, true);
  }

  async upload(entry: ClipboardEntry): Promise<void> {
    // A manual upload can be the first time the server learns about this entry.
    // Without the reference, garbage collection would reclaim the contents that
    // were just uploaded.
    this.#publishEntry(entry);
    await this.#uploadEntry(entry, MANUAL_UPLOAD_LIMIT, true);
  }

  async fetchEntries(entryIds: readonly string[]): Promise<ClipboardEntry[]> {
    const entries: ClipboardEntry[] = [];
    for (let index = 0; index < entryIds.length; index += ENTRY_FETCH_BATCH) {
      entries.push(
        ...await this.#fetchEntryBatch(entryIds.slice(index, index + ENTRY_FETCH_BATCH)),
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

  #publishEntry(entry: ClipboardEntry): boolean {
    return this.#send({
        type: "clipboard.publish",
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
          mime: file.mime,
          available: file.available,
        })),
        sourceDeviceId: entry.sourceDeviceId,
        createdAt: entry.createdAt,
        pinned: entry.pinned,
      },
    });
  }

  async #publishAndConfirm(entry: ClipboardEntry): Promise<void> {
    while (!this.#stopped) {
      await this.#waitForConnection();
      try {
        await this.#waitForPublishConfirmation(entry);
        return;
      } catch (error) {
        if (!this.#isRecoverableUploadError(error)) throw error;
      }
    }
    throw new Error("同步连接已断开");
  }

  #waitForPublishConfirmation(entry: ClipboardEntry): Promise<void> {
    const entryId = entry.id;
    return new Promise<void>((resolve, reject) => {
      const timer = window.setTimeout(
        () => this.#rejectPublishConfirmation(entryId, "剪贴板同步确认超时"),
        30_000,
      );
      this.#publishConfirmations.set(entryId, { resolve, reject, timer });
      if (!this.#publishEntry(entry)) {
        this.#rejectPublishConfirmation(entryId, "同步服务未连接");
      }
    });
  }

  #resolvePublishConfirmation(entry: ClipboardEntry): void {
    const entryId = entry.id;
    const confirmation = this.#publishConfirmations.get(entryId);
    if (!confirmation) return;
    window.clearTimeout(confirmation.timer);
    this.#publishConfirmations.delete(entryId);
    confirmation.resolve();
  }

  #rejectPublishConfirmation(entryId: string, message: string): void {
    const confirmation = this.#publishConfirmations.get(entryId);
    if (!confirmation) return;
    window.clearTimeout(confirmation.timer);
    this.#publishConfirmations.delete(entryId);
    confirmation.reject(new Error(message));
  }

  async #fetchEntryBatch(entryIds: readonly string[]): Promise<ClipboardEntry[]> {
    if (!entryIds.length) return [];
    await this.#waitForConnection();
    const requestId = crypto.randomUUID();
    return new Promise<ClipboardEntry[]>((resolve, reject) => {
      const timer = window.setTimeout(
        () => this.#rejectEntryFetch(requestId, "获取远端历史超时"),
        30_000,
      );
      this.#entryFetches.set(requestId, { resolve, reject, timer });
      if (!this.#send({ type: "clipboard.fetch", requestId, entryIds: [...entryIds] })) {
        this.#rejectEntryFetch(requestId, "同步服务未连接");
      }
    });
  }

  #resolveEntryFetch(requestId: string, entries: ClipboardEntry[]): void {
    const fetch = this.#entryFetches.get(requestId);
    if (!fetch) return;
    window.clearTimeout(fetch.timer);
    this.#entryFetches.delete(requestId);
    fetch.resolve(entries);
  }

  #rejectEntryFetch(requestId: string, message: string): void {
    const fetch = this.#entryFetches.get(requestId);
    if (!fetch) return;
    window.clearTimeout(fetch.timer);
    this.#entryFetches.delete(requestId);
    fetch.reject(new Error(message));
  }

  async downloadFile(entry: ClipboardEntry, file: ClipboardFile): Promise<void> {
    const transferId = crypto.randomUUID();
    await invoke("begin_file_download", {
      transferId,
      fileId: file.fileId,
      expectedSize: file.size,
    });
    try {
      const completed = this.#waitForTransfer(transferId, entry.id, file);
      if (!this.#send({
        type: "file.download",
        transferId,
        entryId: entry.id,
        fileId: file.fileId,
        sourceDeviceId: entry.sourceDeviceId,
      })) {
        this.#rejectTransfer(transferId, "同步服务未连接");
      }
      await completed;
    } catch (error) {
      await invoke("cancel_file_download", { transferId }).catch(() => undefined);
      throw error;
    }
  }

  delete(entryId: string): void {
    this.#send({ type: "clipboard.delete", entryId });
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
        await this.#uploadFileOnce(entry, file, onProgress);
        return;
      } catch (error) {
        if (this.#stopped || !this.#isRecoverableUploadError(error)) throw error;
        await this.#waitForConnection();
      }
    }
    throw new Error("同步连接已断开");
  }

  async #uploadFileOnce(
    entry: ClipboardEntry,
    file: ClipboardFile,
    onProgress: (uploadedBytes: number) => void,
  ): Promise<void> {
    const transferId = crypto.randomUUID();
    const completed = this.#waitForTransfer(transferId);
    const ready = this.#waitForUploadReady(transferId);
    if (!this.#send({
      type: "file.upload.begin",
      transferId,
      fileId: file.fileId,
      size: file.size,
      mime: file.mime,
    })) {
      this.#rejectTransfer(transferId, "同步服务未连接");
      this.#rejectUploadReady(transferId, "同步服务未连接");
      await completed;
      throw new Error("同步服务未连接");
    }
    try {
      const upload = await ready;
      // The server already had these bytes, so the transfer is over before it
      // began — this is what makes copying a folder twice nearly free.
      if (upload.stored) {
        onProgress(file.size);
        await completed;
        return;
      }
      const resumeOffset = upload.offset;
      if (resumeOffset > file.size) throw new Error("服务器续传偏移超出文件大小");
      onProgress(resumeOffset);
      let uploadedBytes = resumeOffset;
      for (let offset = resumeOffset; offset < file.size; offset += FILE_CHUNK_SIZE) {
        const length = Math.min(FILE_CHUNK_SIZE, file.size - offset);
        const data = await invoke<string>("read_file_chunk", {
          entryId: entry.id, fileId: file.fileId, offset, length,
        });
        if (!this.#send({ type: "file.chunk", transferId, data })) {
          this.#rejectTransfer(transferId, "同步服务未连接");
          await completed;
          throw new Error("同步服务未连接");
        }
        const padding = data.endsWith("==") ? 2 : data.endsWith("=") ? 1 : 0;
        uploadedBytes += (data.length / 4) * 3 - padding;
        onProgress(uploadedBytes);
      }
      if (!this.#send({ type: "file.complete", transferId })) {
        this.#rejectTransfer(transferId, "同步服务未连接");
      }
      await completed;
    } catch (error) {
      await completed.catch(() => undefined);
      throw error;
    }
  }

  #waitForTransfer(transferId: string, entryId?: string, file?: ClipboardFile): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      const timer = window.setTimeout(
        () => this.#rejectTransfer(transferId, "文件传输超时"),
        5 * 60_000,
      );
      this.#pending.set(transferId, {
        resolve, reject, timer, writeChain: Promise.resolve(), entryId, file,
      });
    });
  }

  #waitForUploadReady(transferId: string): Promise<UploadReady> {
    return new Promise<UploadReady>((resolve, reject) => {
      const timer = window.setTimeout(
        () => {
          this.#rejectUploadReady(transferId, "服务器未接受文件上传");
          this.#rejectTransfer(transferId, "服务器未接受文件上传");
        },
        15_000,
      );
      this.#uploadReady.set(transferId, { resolve, reject, timer });
    });
  }

  #resolveUploadReady(transferId: string, ready: UploadReady): void {
    const transfer = this.#uploadReady.get(transferId);
    if (!transfer) return;
    window.clearTimeout(transfer.timer);
    this.#uploadReady.delete(transferId);
    transfer.resolve(ready);
  }

  #rejectUploadReady(transferId: string, message: string): void {
    const transfer = this.#uploadReady.get(transferId);
    if (!transfer) return;
    window.clearTimeout(transfer.timer);
    this.#uploadReady.delete(transferId);
    transfer.reject(new Error(message));
  }

  #resolveTransfer(transferId: string): void {
    const transfer = this.#pending.get(transferId);
    if (!transfer) return;
    window.clearTimeout(transfer.timer);
    this.#pending.delete(transferId);
    transfer.resolve();
  }

  #rejectTransfer(transferId: string, message: string): void {
    const transfer = this.#pending.get(transferId);
    if (!transfer) return;
    window.clearTimeout(transfer.timer);
    this.#pending.delete(transferId);
    transfer.reject(new Error(message));
  }

  async #serveSourceFile(transferId: string, entryId: string, fileId: string): Promise<void> {
    try {
      for (let offset = 0; ; offset += FILE_CHUNK_SIZE) {
        const data = await invoke<string>("read_file_chunk", {
          entryId, fileId, offset, length: FILE_CHUNK_SIZE,
        });
        if (!data) break;
        if (!this.#send({ type: "file.chunk", transferId, data })) return;
      }
      this.#send({ type: "file.complete", transferId });
    } catch (error) {
      const message = `无法读取源文件：${error instanceof Error ? error.message : String(error)}`;
      this.#send({ type: "file.abort", transferId, message });
      this.handlers.onError(message);
    }
  }

  async #handleMessage(data: unknown): Promise<void> {
    const result = ServerMessageSchema.safeParse(JSON.parse(String(data)));
    if (!result.success) return;
    const message = result.data;
    switch (message.type) {
      case "file.uploaded":
        // Sent either instead of `file.upload.ready` (instant upload) or after
        // the last chunk; resolving a ready waiter that is already gone is a
        // no-op, so both cases share this branch.
        this.#resolveUploadReady(message.transferId, { stored: true });
        this.#resolveTransfer(message.transferId);
        return;
      case "file.available":
        this.handlers.onFileAvailable(message.fileId);
        return;
      case "file.upload.ready":
        this.#resolveUploadReady(message.transferId, { stored: false, offset: message.offset });
        return;
      case "file.failed":
        this.#rejectUploadReady(message.transferId, message.message);
        this.#rejectTransfer(message.transferId, message.message);
        return;
      case "file.source.request":
        await this.#serveSourceFile(message.transferId, message.entryId, message.fileId);
        return;
      case "file.chunk": {
        const transfer = this.#pending.get(message.transferId);
        if (!transfer?.entryId || !transfer.file) return;
        transfer.writeChain = transfer.writeChain?.then(() => invoke("append_file_download", {
          transferId: message.transferId,
          data: message.data,
        }));
        return;
      }
      case "file.complete": {
        const transfer = this.#pending.get(message.transferId);
        if (!transfer?.entryId || !transfer.file) return;
        try {
          await transfer.writeChain;
          await invoke("finish_file_download", { transferId: message.transferId });
          this.#resolveTransfer(message.transferId);
        } catch (error) {
          this.#rejectTransfer(message.transferId, String(error));
        }
        return;
      }
      case "auth.ack":
        this.#markConnected();
        this.handlers.onConnected(true);
        this.handlers.onManifest(message.manifest, message.devices);
        return;
      case "clipboard.entries":
        this.#resolveEntryFetch(message.requestId, message.entries);
        return;
      case "clipboard.created":
        this.#resolvePublishConfirmation(message.entry);
        this.handlers.onEntry(message.entry);
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
    const socket = new WebSocket(this.url);
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
      this.#rejectActiveTransfers("同步连接已断开");
      this.handlers.onConnected(false);
      if (!this.#stopped) {
        this.#reconnectTimer = window.setTimeout(() => this.#open(), 2500);
      }
    });

    socket.addEventListener("error", () => socket.close());
  }
}
