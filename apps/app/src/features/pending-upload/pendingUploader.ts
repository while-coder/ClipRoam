import type { ClipboardEntry, Device, EntryPublishInput, EntryPublishRequest } from "@cliproam/protocol";
import { isTransientNetworkError, type SyncRequester } from "../sync/syncHttp";
import { errorMessage } from "../../utils/error";
import { FileUploader, type FileUploaderDeps } from "./fileUploader";
const ENTRY_HTTP_TIMEOUT_MS = 30_000;
const QUEUE_FAILURE_BACKOFF_MS = 60_000;
const QUEUE_FAILURE_LIMIT = 3;
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
  #queueFailures = new Map<number, { at: number; count: number }>();
  #skippedRows = new Set<number>();
  #files: FileUploader;
  constructor(private readonly http: SyncRequester, private readonly device: Device, private readonly deps: PendingUploaderDeps, private autoUploadLimit: number) {
    this.#files = new FileUploader(http, deps);
  }
  start(): void {
    void this.deps.session.start(() => this.#drainLoop()).catch(() => undefined);
  }
  retrySkipped(): void { this.#skippedRows.clear(); }
  setAutoUploadLimit(limitBytes: number): void { this.autoUploadLimit = limitBytes; }
  // The resident drain loop: the durable capture queue is the single replay
  // mechanism — captures land there with their full payload, and this loop
  // publishes them strictly in insertion order on a fixed pulse. A row that
  // cannot proceed right now (recent failure backoff, lost HTTP) waits for a
  // later pulse; a row that failed three times is skipped for this session so
  // it cannot block the rows behind it — reconnecting gives it another chance.
  async #drainLoop(): Promise<void> {
    while (!this.deps.session.signal.aborted) {
      await this.deps.session.delay(DRAIN_POLL_INTERVAL_MS);
      const row = await this.deps.session.invoke<PendingQueueRow | null>("peek_pending_entry", {
        skipSeqs: this.#skippedRows.size ? [...this.#skippedRows] : null,
      }).catch(() => null);
      if (!row) continue;
      const failure = this.#queueFailures.get(row.seq);
      if (failure && Date.now() - failure.at < QUEUE_FAILURE_BACKOFF_MS) continue;
      try {
        await this.#publishQueueRow(row);
        this.#queueFailures.delete(row.seq);
      } catch (error) {
        if (this.deps.session.signal.aborted) return;
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
          this.deps.onError(
            `剪贴板记录同步失败，已暂时跳过：${errorMessage(error)}`,
          );
          continue;
        }
        this.#queueFailures.set(row.seq, { at: Date.now(), count: attempts });
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
