import { createHash, type Hash } from "node:crypto";
import { appendFileSync, closeSync, openSync, readSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { FILE_CHUNK_SIZE, type ClientMessage } from "@cliproam/protocol";
import type { ClientConnection, SendMessage } from "../app/Connection.js";
import type { ServerConfig } from "../app/ServerConfig.js";
import type { FileStore } from "./FileStore.js";

type UploadBegin = Extract<ClientMessage, { type: "file.upload.begin" }>;
type Upload = {
  client: ClientConnection;
  userId: string;
  fileId: string;
  mime?: string;
  expectedSize: number;
  receivedSize: number;
  hash: Hash;
  temporaryPath: string;
  finalPath: string;
};

// Uploads are addressed by the sha256 of their content, which makes the server
// side of a transfer stateless with respect to entries and devices: the same
// bytes are always the same target, so a resumed upload — even from a different
// device or a renamed file — continues the same partial file.
export class FileUploadService {
  #uploads = new Map<string, Upload>();
  #waiters = new Map<string, { client: ClientConnection; fileId: string }>();

  constructor(
    private readonly files: FileStore,
    private readonly config: ServerConfig,
    private readonly send: SendMessage,
  ) {}

  begin(client: ClientConnection, message: UploadBegin): void {
    if (message.size >= this.config.maxStoredFileBytes) {
      this.send(client, { type: "file.failed", transferId: message.transferId, message: "文件超过服务器存储上限" });
      return;
    }
    // Content the server already holds needs no transfer at all.
    if (this.files.has(message.fileId)) {
      this.send(client, { type: "file.uploaded", transferId: message.transferId, fileId: message.fileId });
      return;
    }

    // A different account may already be filling the same content-addressed
    // target. Waiting avoids concurrent writes to one shared .part file.
    if ([...this.#uploads.values()].some((upload) => upload.fileId === message.fileId)) {
      this.#waiters.set(message.transferId, { client, fileId: message.fileId });
      return;
    }

    const finalPath = this.files.preparePath(message.fileId);
    const temporaryPath = `${finalPath}.part`;
    const receivedSize = this.#preparePartialFile(temporaryPath, message.size);

    this.#uploads.set(message.transferId, {
      client,
      userId: client.userId,
      fileId: message.fileId,
      mime: message.mime,
      expectedSize: message.size,
      receivedSize,
      // Feeding the resumed bytes back in keeps the digest incremental, since a
      // hash state cannot be persisted across connections.
      hash: hashPrefix(temporaryPath, receivedSize),
      temporaryPath,
      finalPath,
    });
    this.send(client, { type: "file.upload.ready", transferId: message.transferId, offset: receivedSize });
  }

  receiveChunk(client: ClientConnection, transferId: string, encodedData: string): boolean {
    const upload = this.#uploads.get(transferId);
    if (upload?.client !== client || upload.userId !== client.userId) return false;

    const data = Buffer.from(encodedData, "base64");
    if (data.length > FILE_CHUNK_SIZE) {
      this.#fail(transferId, "文件分块超过限制", false);
      return true;
    }
    upload.receivedSize += data.length;
    if (upload.receivedSize > upload.expectedSize) {
      this.#fail(transferId, "上传内容超过声明大小", false);
      return true;
    }
    appendFileSync(upload.temporaryPath, data);
    upload.hash.update(data);
    return true;
  }

  complete(client: ClientConnection, transferId: string): boolean {
    const upload = this.#uploads.get(transferId);
    if (upload?.client !== client || upload.userId !== client.userId) return false;
    if (upload.receivedSize !== upload.expectedSize) {
      this.#fail(transferId, "上传内容大小不完整", false);
      return true;
    }
    // The declared hash decides where the content lands, so accepting bytes that
    // do not match it would let one upload poison every reference to that hash.
    if (upload.hash.digest("hex") !== upload.fileId) {
      this.#fail(transferId, "文件内容校验失败", false);
      return true;
    }
    renameSync(upload.temporaryPath, upload.finalPath);
    this.files.store(upload.fileId, upload.expectedSize, upload.mime);
    this.#uploads.delete(transferId);
    this.send(client, { type: "file.uploaded", transferId, fileId: upload.fileId });
    this.#settleWaiters(upload.fileId, true);
    return true;
  }

  fail(client: ClientConnection, transferId: string, message: string): boolean {
    const waiter = this.#waiters.get(transferId);
    if (waiter?.client === client) {
      this.#waiters.delete(transferId);
      return true;
    }
    const upload = this.#uploads.get(transferId);
    if (upload?.client !== client) return false;
    this.#fail(transferId, message, false);
    return true;
  }

  handleClientClose(client: ClientConnection): void {
    for (const [transferId, waiter] of this.#waiters) {
      if (waiter.client === client) this.#waiters.delete(transferId);
    }
    for (const [transferId, upload] of this.#uploads) {
      if (upload.client === client) this.#fail(transferId, "源设备已离线");
    }
  }

  #preparePartialFile(temporaryPath: string, expectedSize: number): number {
    try {
      const partial = statSync(temporaryPath);
      const expired = this.config.resumableUploadTtlMs === 0
        || Date.now() - partial.mtimeMs > this.config.resumableUploadTtlMs;
      if (partial.size <= expectedSize && !expired) return partial.size;
      rmSync(temporaryPath, { force: true });
    } catch {
      // A missing partial file simply starts a new upload.
    }
    writeFileSync(temporaryPath, Buffer.alloc(0));
    return 0;
  }

  #fail(transferId: string, message: string, preservePartial = true): void {
    const upload = this.#uploads.get(transferId);
    if (!upload) return;
    if (!preservePartial) rmSync(upload.temporaryPath, { force: true });
    this.#uploads.delete(transferId);
    this.send(upload.client, { type: "file.failed", transferId, message });
    this.#settleWaiters(upload.fileId, false, message);
  }

  #settleWaiters(fileId: string, uploaded: boolean, message = "相同文件上传失败"): void {
    for (const [transferId, waiter] of this.#waiters) {
      if (waiter.fileId !== fileId) continue;
      this.#waiters.delete(transferId);
      if (uploaded) this.send(waiter.client, { type: "file.uploaded", transferId, fileId });
      else this.send(waiter.client, { type: "file.failed", transferId, message });
    }
  }
}

function hashPrefix(path: string, byteLength: number): Hash {
  const hash = createHash("sha256");
  if (byteLength <= 0) return hash;
  const descriptor = openSync(path, "r");
  try {
    const buffer = Buffer.allocUnsafe(FILE_CHUNK_SIZE);
    let read = 0;
    while (read < byteLength) {
      const bytes = readSync(descriptor, buffer, 0, Math.min(buffer.length, byteLength - read), read);
      if (bytes <= 0) break;
      hash.update(buffer.subarray(0, bytes));
      read += bytes;
    }
  } finally {
    closeSync(descriptor);
  }
  return hash;
}
