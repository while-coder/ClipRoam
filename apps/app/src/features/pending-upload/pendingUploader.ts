import type { ClipboardEntry, Device, EntryPublishInput, EntryPublishRequest } from "@cliproam/protocol";
import type { SyncRequester } from "../sync/syncHttp";
import { errorMessage } from "../../utils/error";
import { FileUploader, type FileUploaderDeps } from "./fileUploader";
const ENTRY_HTTP_TIMEOUT_MS = 30_000;
const DRAIN_POLL_INTERVAL_MS = 2_000;
type PendingUploaderDeps = FileUploaderDeps & {
  onPublished(): void;
};
type PendingQueueRow = {
  seq: number;
  kind: ClipboardEntry["kind"];
  content: string;
  extra: Partial<Pick<ClipboardEntry, "html" | "rtf" | "fileInfo" | "imageInfo">>;
};

/** Captures stay durable in Rust; one row is published at a time. */
export class PendingUploader {
  #files: FileUploader;
  constructor(private readonly http: SyncRequester, private readonly device: Device, private readonly deps: PendingUploaderDeps, private autoUploadLimit: number) {
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
      const row = await this.deps.session.invoke<PendingQueueRow | null>("peek_pending_entry").catch((error) => {
        if (!this.deps.session.signal.aborted) this.deps.onError(`待同步记录读取失败：${errorMessage(error)}`);
        return null;
      });
      if (!row) continue;
      try {
        await this.#publishQueueRow(row);
      } catch (error) {
        if (this.deps.session.signal.aborted) return;
        console.warn(`Pending upload failed: seq=${row.seq} kind=${row.kind}`, error);
        try {
          await this.deps.session.invoke("fail_pending_entry", { seq: row.seq });
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
    await this.#files.uploadEntry({ ...payload, id: `p${row.seq}` } as ClipboardEntry, this.autoUploadLimit);
    await this.#publishEntry(payload);
    this.deps.onPublished();
    await this.deps.session.invoke("dequeue_pending_entry", { seq: row.seq });
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
  async #publishEntry(entry: EntryPublishInput): Promise<void> {
    const request: EntryPublishRequest = {
      entry,
    };
    await this.http.request(
      "POST",
      "/entries",
      this.#jsonInit(request, ENTRY_HTTP_TIMEOUT_MS),
      null,
      "",
    );
  }

}
