import { type ClientMessage } from "@cliproam/protocol";
import type { ClientConnection, SendMessage } from "../app/Connection.js";
import type { ServerConfig } from "../app/ServerConfig.js";
import type { FileStore } from "./FileStore.js";
import { FileDownloadService } from "./FileDownloadService.js";
import { FileUploadService } from "./FileUploadService.js";

type UploadBegin = Extract<ClientMessage, { type: "file.upload.begin" }>;
type DownloadRequest = Extract<ClientMessage, { type: "file.download" }>;

// Keeps WebSocket protocol dispatch in one place while upload and download
// lifecycles remain independent and easy to read.
export class FileTransferService {
  readonly #uploads: FileUploadService;
  readonly #downloads: FileDownloadService;

  constructor(
    files: FileStore,
    canRead: (userId: string, entryId: string, fileId: string) => boolean,
    config: ServerConfig,
    private readonly send: SendMessage,
  ) {
    this.#uploads = new FileUploadService(files, config, send);
    this.#downloads = new FileDownloadService(files, canRead, send);
  }

  beginUpload(client: ClientConnection, message: UploadBegin): void {
    this.#uploads.begin(client, message);
  }

  downloadFile(
    client: ClientConnection,
    message: DownloadRequest,
    clients: ReadonlySet<ClientConnection>,
  ): Promise<void> {
    return this.#downloads.download(client, message, clients);
  }

  receiveChunk(client: ClientConnection, transferId: string, encodedData: string): void {
    if (this.#uploads.receiveChunk(client, transferId, encodedData)) return;
    if (this.#downloads.receiveChunk(client, transferId, encodedData)) return;
    this.send(client, { type: "file.failed", transferId, message: "文件传输不存在或已过期" });
  }

  completeTransfer(client: ClientConnection, transferId: string): void {
    if (this.#uploads.complete(client, transferId)) return;
    if (this.#downloads.complete(client, transferId)) return;
    this.send(client, { type: "file.failed", transferId, message: "文件传输不存在或已过期" });
  }

  failTransfer(client: ClientConnection, transferId: string, message: string): void {
    if (this.#uploads.fail(client, transferId, message)) return;
    this.#downloads.fail(client, transferId, message);
  }

  handleClientClose(client: ClientConnection): void {
    this.#uploads.handleClientClose(client);
    this.#downloads.handleClientClose(client);
  }
}
