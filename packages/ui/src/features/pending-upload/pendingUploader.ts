import { errorMessage } from "../../utils";
import { EntryPublishResponseSchema, type ClipboardEntry, type Device, type EntryPublishInput, type EntryPublishRequest } from "@cliproam/protocol";
import type { SyncRequester } from "../sync/httpClient";
import { FileUploader, type FileUploaderDeps } from "./fileUploader";
import type { PendingRow } from "../../types";
const ENTRY_HTTP_TIMEOUT_MS = 30_000;
const DRAIN_POLL_INTERVAL_MS = 2_000;
/** Captures stay durable in IndexedDB; one row is published at a time. */
export class PendingUploader {
  #files: FileUploader;
  constructor(private readonly http: SyncRequester, private readonly device: Device, private readonly deps: FileUploaderDeps, private autoUploadLimit: number) {
    this.#files = new FileUploader(http, deps);
  }
  start(): void {
    void this.deps.session.start(() => this.#drainLoop()).catch(() => undefined);
  }
  setAutoUploadLimit(limitBytes: number): void { this.autoUploadLimit = limitBytes; }
  // Only waiting rows are processed; a failed row stays in the durable queue
  // until its status is manually changed back to wait.
  async #drainLoop(): Promise<void> {
    while (!this.deps.session.signal.aborted) {
      await this.deps.session.delay(DRAIN_POLL_INTERVAL_MS);
      const row = await this.deps.session.pendingUploads.rows().then((rows) => {
        const first = rows.find((row) => row.status === "wait");
        return first?.resolving ? undefined : first;
      }).catch((error) => {
        if (!this.deps.session.signal.aborted) this.deps.onError(`待同步记录读取失败：${errorMessage(error)}`);
        return null;
      });
      if (!row) continue;
      try {
        await this.#publishQueueRow(row);
      } catch (error) {
        if (this.deps.session.signal.aborted) return;
        console.warn(`Pending upload failed: seq=${row.seq} kind=${row.entry.kind}`, error);
        try {
          await this.deps.session.pendingUploads.setStatus(row.seq, "failed");
          this.#refreshPending();
        } catch (statusError) {
          if (!this.deps.session.signal.aborted) this.deps.onError(`同步失败状态保存失败：${errorMessage(statusError)}`);
          return;
        }
        this.deps.onError(`剪贴板记录同步失败：${errorMessage(error)}`);
      }
    }
  }

  // Upload contents before publishing metadata: any file failure keeps the row
  // pending without creating or updating the server entry. Content-addressed
  // uploads need no entry id and are reused if publishing must be retried.
  // Only a confirmed publish refreshes history and lets the row leave the queue.
  async #publishQueueRow(row: PendingRow): Promise<void> {
    const payload: EntryPublishInput = {
      kind: row.entry.kind,
      content: row.entry.content,
      html: row.entry.html ?? undefined,
      rtf: row.entry.rtf ?? undefined,
      fileInfo: row.entry.fileInfo ?? undefined,
      imageInfo: row.entry.imageInfo ?? undefined,
      sourceDeviceId: this.device.id,
    };
    await this.#files.uploadEntry({ ...payload, id: `p${row.seq}` } as ClipboardEntry, this.autoUploadLimit);
    const entryId = await this.#publishEntry(payload);
    await this.deps.session.history.recordPublished(entryId, row.entry);
    await this.deps.session.pendingUploads.complete(row.seq);
    this.#refreshPending();
  }

  #refreshPending(): void {
    void this.deps.session.pendingUploads.refresh().catch((error) => {
      if (!this.deps.session.signal.aborted) this.deps.onError(`待上传记录读取失败：${errorMessage(error)}`);
    });
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
  async #publishEntry(entry: EntryPublishInput): Promise<string> {
    const request: EntryPublishRequest = {
      entry,
    };
    const result = await this.http.request(
      "POST",
      "/entries",
      this.#jsonInit(request, ENTRY_HTTP_TIMEOUT_MS),
      EntryPublishResponseSchema,
      "服务器返回了不兼容的发布响应",
    );
    return result!.entry.id;
  }

}
