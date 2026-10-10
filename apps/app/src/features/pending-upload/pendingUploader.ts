import { EntryPublishResponseSchema, type ClipboardEntry, type Device, type EntryPublishInput, type EntryPublishRequest } from "@cliproam/protocol";
import { invoke } from "@tauri-apps/api/core";
import { isTransientNetworkError, type SyncRequester } from "../sync/syncHttp";
import { errorMessage } from "../../utils/error";
import { FileUploader, type FileUploaderDeps } from "./fileUploader";
const ENTRY_HTTP_TIMEOUT_MS = 30_000;
const QUEUE_FAILURE_BACKOFF_MS = 60_000;
const QUEUE_FAILURE_LIMIT = 3;
const DRAIN_POLL_INTERVAL_MS = 2_000;
type PendingUploaderDeps = FileUploaderDeps & {
  onPublished(): void;
  activate(entryId: string): Promise<ClipboardEntry>;
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
  start(): void { void this.#drainLoop(); }
  retrySkipped(): void { this.#skippedRows.clear(); }
  setAutoUploadLimit(limitBytes: number): void { this.autoUploadLimit = limitBytes; }
  // The resident drain loop: the durable capture queue is the single replay
  // mechanism — captures land there with their full payload, and this loop
  // publishes them strictly in insertion order on a fixed pulse. A row that
  // cannot proceed right now (recent failure backoff, lost HTTP) waits for a
  // later pulse; a row that failed three times is skipped for this session so
  // it cannot block the rows behind it — reconnecting gives it another chance.
  async #drainLoop(): Promise<void> {
    while (!this.deps.isStopped()) {
      await new Promise((resolve) => window.setTimeout(resolve, DRAIN_POLL_INTERVAL_MS));
      if (this.deps.isStopped()) return;
      const row = await invoke<PendingQueueRow | null>("peek_pending_entry", {
        skipSeqs: this.#skippedRows.size ? [...this.#skippedRows] : null,
      }).catch(() => null);
      if (this.deps.isStopped()) return;
      if (!row) continue;
      const failure = this.#queueFailures.get(row.seq);
      if (failure && Date.now() - failure.at < QUEUE_FAILURE_BACKOFF_MS) continue;
      try {
        await this.#publishQueueRow(row);
        this.#queueFailures.delete(row.seq);
      } catch (error) {
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

  // Publishes one queue row: the metadata goes first (other devices can start
  // pulling while the contents upload), then the contents — the upload HTTP is
  // content-addressed and needs no entry id — then the broadcast activation,
  // and the row leaves the queue. Refresh on HTTP confirmation immediately;
  // only detail queries populate the local history cache.
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
    if (this.deps.isStopped()) return;
    this.deps.onPublished();
    await this.#files.uploadEntry({ ...payload, id: `p${row.seq}` } as ClipboardEntry, this.autoUploadLimit);
    if (this.deps.isStopped()) return;
    if (stored.kind !== "files") {
      await this.deps.activate(stored.id).catch(() => undefined);
    }
    if (this.deps.isStopped()) return;
    await invoke("dequeue_pending_entry", { seq: row.seq }).catch(() => undefined);
  }

  #jsonInit(request: unknown, timeoutMs: number): RequestInit {
    return {
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request),
      signal: AbortSignal.timeout(timeoutMs),
    };
  }

  // Every write returns the server's stored entry: its id and timestamp are
  // server-assigned, and the caller must adopt it into local state.

  // The HTTP response is the confirmation the socket echo used to be. The
  // queue-row payload carries no id and no createdAt: identity and timestamp
  // belong to the server, which dedupes by content either way.
  async #publishEntry(entry: EntryPublishInput): Promise<ClipboardEntry> {
    const request: EntryPublishRequest = {
      deviceId: this.device.id,
      entry,
    };
    const stored = await this.http.request(
      "POST",
      "/entries",
      this.#jsonInit(request, ENTRY_HTTP_TIMEOUT_MS),
      EntryPublishResponseSchema,
      "服务器返回了不兼容的发布响应",
    );
    return stored!.entry;
  }

}
