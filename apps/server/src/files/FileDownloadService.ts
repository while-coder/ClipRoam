import { createReadStream } from "node:fs";
import { FILE_CHUNK_SIZE, type ClientMessage } from "@cliproam/protocol";
import type { ClientConnection, SendMessage } from "../app/Connection.js";
import type { FileStoreResolver } from "./FileStore.js";

type DownloadRequest = Extract<ClientMessage, { type: "file.download" }>;
type Relay = { source: ClientConnection; target: ClientConnection; userId: string };

export class FileDownloadService {
  #relays = new Map<string, Relay>();

  constructor(
    private readonly files: FileStoreResolver,
    private readonly send: SendMessage,
  ) {}

  async download(
    client: ClientConnection,
    message: DownloadRequest,
    clients: ReadonlySet<ClientConnection>,
  ): Promise<void> {
    const stored = this.files(client.userId).get(message.fileId);
    if (stored) {
      try {
        for await (const chunk of createReadStream(stored.path, { highWaterMark: FILE_CHUNK_SIZE })) {
          this.send(client, {
            type: "file.chunk",
            transferId: message.transferId,
            data: Buffer.from(chunk).toString("base64"),
          });
        }
        this.send(client, { type: "file.complete", transferId: message.transferId });
      } catch {
        this.send(client, { type: "file.failed", transferId: message.transferId, message: "服务器文件已不可用" });
      }
      return;
    }

    const source = [...clients].find((candidate) =>
      candidate.userId === client.userId && candidate.device.id === message.sourceDeviceId);
    if (!source) {
      this.send(client, { type: "file.failed", transferId: message.transferId, message: "源设备不在线，无法获取此文件" });
      return;
    }
    this.#relays.set(message.transferId, { source, target: client, userId: client.userId });
    this.send(source, {
      type: "file.source.request",
      transferId: message.transferId,
      entryId: message.entryId,
      fileId: message.fileId,
    });
  }

  receiveChunk(client: ClientConnection, transferId: string, encodedData: string): boolean {
    const relay = this.#relays.get(transferId);
    if (relay?.source !== client || relay.userId !== client.userId) return false;
    if (Buffer.from(encodedData, "base64").length > FILE_CHUNK_SIZE) {
      this.#failRelay(transferId, "文件分块超过限制");
      return true;
    }
    this.send(relay.target, { type: "file.chunk", transferId, data: encodedData });
    return true;
  }

  complete(client: ClientConnection, transferId: string): boolean {
    const relay = this.#relays.get(transferId);
    if (relay?.source !== client || relay.userId !== client.userId) return false;
    this.send(relay.target, { type: "file.complete", transferId });
    this.#relays.delete(transferId);
    return true;
  }

  fail(client: ClientConnection, transferId: string, message: string): boolean {
    const relay = this.#relays.get(transferId);
    if (!relay || (relay.source !== client && relay.target !== client)) return false;
    this.#failRelay(transferId, message, client);
    return true;
  }

  handleClientClose(client: ClientConnection): void {
    for (const [transferId, relay] of this.#relays) {
      if (relay.source === client || relay.target === client) {
        this.#failRelay(transferId, "文件传输设备已离线", client);
      }
    }
  }

  #failRelay(transferId: string, message: string, sender?: ClientConnection): void {
    const relay = this.#relays.get(transferId);
    if (!relay) return;
    const target = sender === relay.source ? relay.target : relay.source;
    this.send(target, { type: "file.failed", transferId, message });
    this.#relays.delete(transferId);
  }
}
