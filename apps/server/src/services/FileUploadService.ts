import { randomUUID } from "node:crypto";
import { appendFileSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { FILE_CHUNK_SIZE, type ClientMessage } from "@cliproam/protocol";
import type { ClientConnection, SendMessage } from "../app/Connection.js";
import type { ServerConfig } from "../app/ServerConfig.js";
import { ClipRoamStore } from "../storage/ClipRoamStore.js";

type UploadBegin = Extract<ClientMessage, { type: "file.upload.begin" }>;
type Upload = {
  client: ClientConnection;
  userId: string;
  deviceId: string;
  entryId: string;
  fileId: string;
  fileFullPath: string;
  name: string;
  expectedSize: number;
  receivedSize: number;
  temporaryPath: string;
  finalPath: string;
};

export class FileUploadService {
  #uploads = new Map<string, Upload>();

  constructor(
    private readonly store: ClipRoamStore,
    private readonly config: ServerConfig,
    private readonly send: SendMessage,
  ) {}

  begin(client: ClientConnection, message: UploadBegin): void {
    if (message.file.size >= this.config.maxStoredFileBytes) {
      this.send(client, { type: "file.failed", transferId: message.transferId, message: "文件超过服务器存储上限" });
      return;
    }
    const entryId = this.store.entryIdForClientId(client.userId, message.clientId);
    if (!entryId) {
      this.send(client, { type: "file.failed", transferId: message.transferId, message: "剪贴板记录尚未创建" });
      return;
    }

    const existing = this.store.getUploadSession(client.userId, client.device.id, message.fileFullPath);
    const matchesExistingFile = existing
      && existing.fileSize === message.file.size
      && existing.fileModifiedAt === message.fileModifiedAt;
    if (existing && !matchesExistingFile) {
      const staleFileId = this.store.deleteUploadSession(client.userId, client.device.id, message.fileFullPath);
      if (staleFileId) rmSync(`${this.store.filePath(client.userId, staleFileId)}.part`, { force: true });
    }

    const fileId = matchesExistingFile ? existing.fileId : randomUUID();
    const finalPath = this.store.filePath(client.userId, fileId);
    const temporaryPath = `${finalPath}.part`;
    this.#cancelActiveUpload(client.userId, fileId);
    const receivedSize = this.#preparePartialFile(temporaryPath, message.file.size);
    this.store.saveUploadSession(client.userId, client.device.id, message.fileFullPath, entryId, {
      fileId,
      fileSize: message.file.size,
      fileModifiedAt: message.fileModifiedAt,
    });

    this.#uploads.set(message.transferId, {
      client,
      userId: client.userId,
      deviceId: client.device.id,
      entryId,
      fileId,
      fileFullPath: message.fileFullPath,
      name: message.file.name,
      expectedSize: message.file.size,
      receivedSize,
      temporaryPath,
      finalPath,
    });
    this.send(client, { type: "file.upload.ready", transferId: message.transferId, fileId, offset: receivedSize });
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
    return true;
  }

  complete(client: ClientConnection, transferId: string): boolean {
    const upload = this.#uploads.get(transferId);
    if (upload?.client !== client || upload.userId !== client.userId) return false;
    if (upload.receivedSize !== upload.expectedSize) {
      this.#fail(transferId, "上传内容大小不完整", false);
      return true;
    }
    renameSync(upload.temporaryPath, upload.finalPath);
    this.store.storeFile(upload.userId, upload.entryId, upload.fileId, {
      path: upload.finalPath,
      size: upload.expectedSize,
      name: upload.name,
    });
    this.store.deleteUploadSession(upload.userId, upload.deviceId, upload.fileFullPath);
    this.#uploads.delete(transferId);
    this.send(client, { type: "file.uploaded", transferId, fileId: upload.fileId });
    return true;
  }

  fail(client: ClientConnection, transferId: string, message: string): boolean {
    const upload = this.#uploads.get(transferId);
    if (upload?.client !== client) return false;
    this.#fail(transferId, message, false);
    return true;
  }

  handleClientClose(client: ClientConnection): void {
    for (const [transferId, upload] of this.#uploads) {
      if (upload.client === client) this.#fail(transferId, "源设备已离线");
    }
  }

  removeEntry(userId: string, entryId: string): void {
    const partialFileIds = this.store.deleteUploadSessionsForEntry(userId, entryId);
    for (const [transferId, upload] of this.#uploads) {
      if (upload.userId === userId && upload.entryId === entryId) {
        this.#fail(transferId, "剪贴板记录已删除", false);
      }
    }
    for (const fileId of partialFileIds) rmSync(`${this.store.filePath(userId, fileId)}.part`, { force: true });
  }

  #cancelActiveUpload(userId: string, fileId: string): void {
    for (const [transferId, upload] of this.#uploads) {
      if (upload.userId === userId && upload.fileId === fileId) {
        this.#fail(transferId, "文件上传已在其他连接中重新开始");
      }
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
  }
}
