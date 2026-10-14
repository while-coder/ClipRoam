import { ref } from "vue";
import type { AccountSession } from "../../accountSession";
import type { LocalClipboardEntry, UploadProgress, PendingRow, StoredEntry } from "../../types";
import { summarize } from "../../utils";
import { entryContents } from "@cliproam/protocol";

/** Shared pending queue for one account. Capture, publishing and views use this instance. */
export class PendingUploadManager {
  readonly entries = ref<LocalClipboardEntry[]>([]);
  readonly count = ref(0);
  readonly progress = ref<Record<string, UploadProgress>>({});
  #progress = new Map<string, UploadProgress>();
  #listRevision = 0;
  #countRevision = 0;

  constructor(private readonly session: AccountSession) {}

  async rows(): Promise<PendingRow[]> {
    return this.session.database.list<PendingRow>("pending");
  }

  get(id: number): Promise<PendingRow | undefined> {
    return this.session.database.get<PendingRow>("pending", id);
  }

  /** File resolution preserves a retry/status change made while hashing. */
  updateResolved(row: PendingRow, failed = false): Promise<void> {
    return this.session.database.update<PendingRow>("pending", row.id, (current) =>
      current ? { ...row, status: failed ? "failed" : current.status } : undefined);
  }

  async complete(id: number): Promise<void> {
    await this.session.database.delete("pending", id);
    this.finishProgress(`p${id}`);
  }

  enqueue(entry: StoredEntry, captureId?: string, resolving = false): Promise<number> {
    return this.session.database.enqueuePending(entry, captureId, resolving);
  }

  setStatus(id: number, status: "wait" | "failed"): Promise<void> {
    return this.session.database.update<PendingRow>("pending", id, (row) => row ? { ...row, status } : undefined);
  }

  async refresh(list = true): Promise<void> {
    const listRevision = list ? ++this.#listRevision : this.#listRevision;
    const countRevision = ++this.#countRevision;
    await this.session.ready;
    const rows = await this.rows();
    if (!list) {
      if (!this.session.signal.aborted && countRevision === this.#countRevision) this.count.value = rows.length;
      return;
    }
    const ids = [...new Set(rows.flatMap((row) => entryContents(row.entry).map((file) => file.fileId)).filter(Boolean))];
    const available = await this.session.files.resolveMany(ids);
    const entries: LocalClipboardEntry[] = [];
    for (const row of rows) entries.push({
      ...await summarize(row.entry, new Set(), async (id) => available.get(id), true), pendingStatus: row.status,
    });
    if (!this.session.signal.aborted) {
      if (listRevision === this.#listRevision) this.entries.value = entries;
      if (countRevision === this.#countRevision) this.count.value = rows.length;
    }
  }

  async remove(entryId: string): Promise<void> {
    if (!/^p\d+$/.test(entryId)) return;
    await this.session.database.delete("pending", Number(entryId.slice(1)));
    this.finishProgress(entryId);
    await this.refresh();
  }

  async retry(entryId: string): Promise<void> {
    if (!/^p\d+$/.test(entryId)) return;
    await this.setStatus(Number(entryId.slice(1)), "wait");
    await this.session.capture.resume();
    await this.refresh();
  }

  queueProgress(entryId: string, uploadedBytes: number, totalBytes: number): void {
    if (this.session.signal.aborted) return;
    this.#progress.set(entryId, { uploadedBytes, totalBytes });
    this.session.schedule("upload-progress", () => {
      const batch = Object.fromEntries(this.#progress);
      this.#progress.clear();
      this.progress.value = { ...this.progress.value, ...batch };
    }, 200);
  }

  finishProgress(entryId: string): void {
    this.#progress.delete(entryId);
    const { [entryId]: _, ...remaining } = this.progress.value;
    this.progress.value = remaining;
  }

  clearProgress(): void {
    this.session.cancelScheduled("upload-progress");
    this.#progress.clear();
    this.progress.value = {};
  }
}
