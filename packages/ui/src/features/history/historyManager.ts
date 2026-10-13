import { ref } from "vue";
import type { ClipboardManifestEntry } from "@cliproam/protocol";
import { HistoryCache } from "./historyCache";
import type { AccountSession } from "../sync/accountSession";
import type { SyncRequester } from "../sync/syncHttp";
import type { EntriesManifestFilter, EntriesManifestPage, LocalClipboardEntry } from "../../types";
import { HistoryClient } from "./historyClient";
import type { StoredEntry } from "../../core/content";

/** Account-wide entry cache and server history operations; views own filters and selection. */
export class HistoryManager extends HistoryCache {
  readonly revision = ref(0);
  #client?: HistoryClient;

  constructor(private readonly session: AccountSession) { super(session.files); }

  connect(http: SyncRequester): void { this.#client = new HistoryClient(http, this.session); }

  invalidate(): void { if (!this.session.signal.aborted) this.revision.value++; }

  fetchHistoryPage(filter: EntriesManifestFilter): Promise<EntriesManifestPage> {
    return this.#client?.fetchHistoryPage(filter) ?? Promise.resolve({ total: 0, entries: [] });
  }

  fetchHistoryInfo(manifest: ClipboardManifestEntry[]): Promise<LocalClipboardEntry[]> {
    if (!this.#client) throw new Error("同步账号未登录");
    return this.#client.fetchHistoryInfo(manifest);
  }

  async getEntry(entryId: string): Promise<LocalClipboardEntry> {
    return (await this.display([entryId], false))[0]!;
  }

  async removeLocal(entryId: string): Promise<void> {
    await this.database.delete("entries", entryId);
    this.invalidate();
  }

  /** Preserve backfilled server metadata and attach sources from the published capture. */
  async recordPublished(entryId: string, entry: StoredEntry): Promise<void> {
    await this.database.update<StoredEntry>("entries", entryId, (previous) => ({
      ...previous ?? entry, id: entryId, sources: entry.sources,
    }));
    this.invalidate();
  }

  async delete(entryId: string): Promise<void> {
    if (!this.#client) throw new Error("同步账号未登录");
    await this.#client.delete(entryId);
    await this.removeLocal(entryId);
  }
}
