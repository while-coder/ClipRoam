import { ref } from "vue";
import { entryContents, type ClipboardEntry, type ClipboardManifestEntry, type FileStatus } from "@cliproam/protocol";
import type { AccountSession } from "../../accountSession";
import type { SyncRequester } from "../sync/httpClient";
import type { EntriesManifestFilter, EntriesManifestPage, LocalClipboardEntry, PendingRow, StoredEntry } from "../../types";
import { HistoryClient } from "./historyClient";
import { summarize } from "../../utils";
import type { AccountDatabase } from "../../platform/database";
import type { AccountFiles } from "../../platform/files";

/** Account-wide entry cache and server history operations; views own filters and selection. */
export class HistoryManager {
  readonly database: AccountDatabase;
  readonly files: AccountFiles;
  readonly revision = ref(0);
  #client?: HistoryClient;

  constructor(private readonly session: AccountSession) {
    this.files = session.files;
    this.database = this.files.database;
  }

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

  async delete(entryIds: readonly string[]): Promise<void> {
    if (!this.#client) throw new Error("同步账号未登录");
    await this.#client.delete(entryIds);
    await this.database.batch(entryIds.map((id) => ({ name: "entries", id })));
    this.invalidate();
  }

  async setFavorite(entryIds: readonly string[], isFavorite: boolean): Promise<void> {
    if (!entryIds.length) return;
    if (!this.#client) throw new Error("同步账号未登录");
    await this.#client.setFavorite(entryIds, isFavorite);
    this.invalidate();
  }

  async get(entryId: string): Promise<StoredEntry> {
    const pending = /^p\d+$/.test(entryId) ? await this.database.get<PendingRow>("pending", Number(entryId.slice(1))) : undefined;
    const entry = pending?.entry ?? await this.database.get<StoredEntry>("entries", entryId);
    if (!entry) throw new Error("剪贴板记录不存在");
    return entry;
  }

  async stale(manifest: ClipboardManifestEntry[]): Promise<string[]> {
    const entries = await Promise.all(manifest.map((item) => this.database.get<StoredEntry>("entries", item.id)));
    return manifest.filter((item, i) => !entries[i] || entries[i]!.version < item.version).map((item) => item.id);
  }

  async upsert(entries: ClipboardEntry[]): Promise<void> {
    for (const entry of entries) await this.database.update<StoredEntry>("entries", entry.id, (previous) => {
      if (previous && previous.version > entry.version) return previous;
      return { ...entry, sources: previous?.sources };
    });
  }

  async unknownFileIds(entryIds: string[]): Promise<string[]> {
    const ids = new Set<string>();
    for (const id of entryIds) for (const file of entryContents(await this.get(id))) if (file.fileId) ids.add(file.fileId);
    const unknown: string[] = [];
    for (const id of ids) if (!(await this.database.get<FileStatus>("files", id))?.stored) unknown.push(id);
    return unknown;
  }

  async upsertFiles(statuses: FileStatus[]): Promise<void> {
    await this.database.batch(statuses.map((value) => ({ name: "files", id: value.fileId, value })));
  }

  async display(entryIds: string[], lightweight = true): Promise<LocalClipboardEntry[]> {
    const entries = await Promise.all(entryIds.map((id) => this.get(id)));
    const ids = [...new Set(entries.flatMap((entry) => entryContents(entry).map((file) => file.fileId)).filter(Boolean))];
    const available = await this.files.resolveMany(ids);
    const results: LocalClipboardEntry[] = [];
    const stored = new Set((await this.database.getMany<FileStatus>("files",ids)).filter((file) => file?.stored).map((file) => file!.fileId));
    for (const entry of entries) results.push(await summarize(entry, stored, async (fileId) => available.get(fileId), lightweight));
    return results;
  }
}
