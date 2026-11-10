import { entryContents, type ClipboardEntry, type EntryCollection, type ClipboardManifestEntry, type FileStatus, type FavoriteFolderInput } from "@cliproam/protocol";
import type { AccountSession } from "../../accountSession";
import type { SyncRequester } from "../sync/httpClient";
import type { EntriesManifestFilter, EntriesManifestPage, LocalClipboardEntry, PendingRow, StoredEntry } from "../../types";
import { HistoryClient } from "./historyClient";
import { summarize } from "../../utils";
import type { AccountDatabase } from "../../platform/database";
import type { AccountFiles } from "../../platform/files";
import { shallowRef } from "vue";

/** Account-wide entry cache and server history operations; views own filters and selection. */
export class HistoryManager {
  readonly database: AccountDatabase;
  readonly files: AccountFiles;
  #listeners = new Set<() => void>();
  #client?: HistoryClient;
  readonly stickyEntries = shallowRef<LocalClipboardEntry[]>([]);
  #stickyVersion = 0;
  #stickyLoadedVersion = -1;
  #stickyRequest?: Promise<LocalClipboardEntry[]>;

  constructor(private readonly session: AccountSession) {
    this.files = session.files;
    this.database = this.files.database;
    session.signal.addEventListener("abort", () => this.#listeners.clear(), { once: true });
  }

  connect(http: SyncRequester): void { this.#client = new HistoryClient(http, this.session); }

  addEvent(listener: () => void): void {
    if (!this.session.signal.aborted) this.#listeners.add(listener);
  }

  delEvent(listener: () => void): void { this.#listeners.delete(listener); }

  invalidate(): void {
    this.#stickyVersion++;
    if (!this.session.signal.aborted) for (const listener of [...this.#listeners]) listener();
  }

  fetchHistoryPage(filter: EntriesManifestFilter): Promise<EntriesManifestPage> {
    return this.#client?.fetchHistoryPage(filter) ?? Promise.resolve({ total: 0, entries: [] });
  }

  fetchFavoritePage(filter: EntriesManifestFilter): Promise<EntriesManifestPage> {
    return this.#client?.fetchFavoritePage(filter) ?? Promise.resolve({ total: 0, entries: [] });
  }

  async fetchHistoryInfo(manifest: ClipboardManifestEntry[]): Promise<LocalClipboardEntry[]> {
    if (!this.#client) throw new Error("同步账号未登录");
    if (manifest[0]?.collection === "sticky") {
      const entries = await this.listSticky(true);
      const byId = new Map(entries.map((entry) => [entry.id, entry]));
      return manifest.flatMap((item) => byId.get(item.id) ?? []);
    }
    return this.#client.fetchHistoryInfo(manifest, manifest[0]?.collection ?? "history");
  }

  async getEntry(entryId: string): Promise<LocalClipboardEntry> {
    return (await this.display([entryId], false))[0]!;
  }

  async removeLocal(entryId: string): Promise<void> {
    await this.database.batch([{ name: "entries", id: entryId }, { name: "favorites", id: entryId }]);
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
    for (const [collection, ids] of await this.#groupByCollection(entryIds)) {
      await this.#client.delete(collection, ids);
    }
    await this.database.batch(entryIds.flatMap((id) => [{ name: "entries" as const, id }, { name: "favorites" as const, id }]));
    this.invalidate();
  }

  async setFavorite(entryIds: readonly string[], isFavorite: boolean, favoriteFolderId?: string | null): Promise<void> {
    if (!entryIds.length) return;
    if (!this.#client) throw new Error("同步账号未登录");
    const ids = isFavorite ? entryIds : (await Promise.all(entryIds.map(async (id) =>
      (await this.get(id)).favoriteId,
    ))).filter((id): id is string => !!id);
    if (!ids.length) return;
    if (isFavorite) {
      for (const [collection, sourceIds] of await this.#groupByCollection(ids)) {
        await this.#client.setFavorite(collection, sourceIds, true, favoriteFolderId);
      }
    } else {
      await this.#client.setFavorite("favorites", ids, false);
    }
    if (!isFavorite) await this.database.batch(ids.map((id) => ({ name: "favorites", id })));
    this.invalidate();
  }

  listFavoriteFolders() {
    if (!this.#client) throw new Error("同步账号未登录");
    return this.#client.listFavoriteFolders();
  }

  listSticky(force = false): Promise<LocalClipboardEntry[]> {
    if (!this.#client) throw new Error("同步账号未登录");
    if (force) this.#stickyVersion++;
    if (this.#stickyRequest) return this.#stickyRequest;
    if (this.#stickyLoadedVersion === this.#stickyVersion) return Promise.resolve(this.stickyEntries.value);
    this.#stickyRequest = (async () => {
      while (true) {
        this.session.signal.throwIfAborted();
        const version = this.#stickyVersion;
        const entries = await this.#client!.listSticky();
        this.session.signal.throwIfAborted();
        if (version !== this.#stickyVersion) continue;
        this.#stickyLoadedVersion = version;
        this.stickyEntries.value = entries;
        return entries;
      }
    })().finally(() => { this.#stickyRequest = undefined; });
    return this.#stickyRequest;
  }

  async setSticky(ids: readonly string[], isSticky: boolean): Promise<void> {
    if (!this.#client) throw new Error("同步账号未登录");
    const targetIds = isSticky ? ids : (await Promise.all(ids.map(async (id) =>
      (await this.get(id)).stickyId,
    ))).filter((id): id is string => !!id);
    if (!targetIds.length) return;
    if (isSticky) {
      for (const [collection, sourceIds] of await this.#groupByCollection(targetIds)) {
        await this.#client.setSticky(collection, sourceIds, true);
      }
    } else {
      await this.#client.setSticky("sticky", targetIds, false);
    }
    this.invalidate();
  }

  async saveFavoriteFolder(input: FavoriteFolderInput, id?: string): Promise<void> {
    if (!this.#client) throw new Error("同步账号未登录");
    await this.#client.saveFavoriteFolder(input, id);
    this.invalidate();
  }

  async deleteFavoriteFolder(id: string): Promise<void> {
    if (!this.#client) throw new Error("同步账号未登录");
    await this.#client.deleteFavoriteFolder(id);
    this.invalidate();
  }

  async #groupByCollection(ids: readonly string[]): Promise<Map<EntryCollection, string[]>> {
    const groups = new Map<EntryCollection, string[]>();
    for (const id of ids) {
      const collection = (await this.get(id)).collection ?? "history";
      const group = groups.get(collection) ?? [];
      group.push(id);
      groups.set(collection, group);
    }
    return groups;
  }

  async get(entryId: string): Promise<StoredEntry> {
    const pending = /^p\d+$/.test(entryId) ? await this.database.get<PendingRow>("pending", Number(entryId.slice(1))) : undefined;
    const entry = pending?.entry
      ?? this.stickyEntries.value.find((entry) => entry.id === entryId)
      ?? await this.database.get<StoredEntry>("entries", entryId)
      ?? await this.database.get<StoredEntry>("favorites", entryId);
    if (!entry) throw new Error("剪贴板记录不存在");
    return entry;
  }

  async stale(manifest: ClipboardManifestEntry[], collection: EntryCollection): Promise<string[]> {
    const store = collection === "favorites" ? "favorites" : "entries";
    const entries = await Promise.all(manifest.map((item) => this.database.get<StoredEntry>(store, item.id)));
    return manifest.filter((item, i) => {
      const entry = entries[i];
      return !entry || !entry.collection || entry.version < item.version || entry.favoriteId === undefined || entry.stickyId === undefined
        || (item.favoriteId !== undefined && entry.favoriteId !== item.favoriteId)
        || (item.stickyId !== undefined && entry.stickyId !== item.stickyId);
    }).map((item) => item.id);
  }

  async upsert(entries: ClipboardEntry[]): Promise<void> {
    for (const entry of entries) {
      if (entry.collection === "sticky") continue;
      await this.database.update<StoredEntry>(entry.collection === "favorites" ? "favorites" : "entries", entry.id, (previous) => {
        if (previous && previous.version > entry.version) return previous;
        return { ...entry, sources: previous?.sources };
      });
    }
  }

  async unknownFileIds(entryIds: string[]): Promise<string[]> {
    return this.unknownEntryFileIds(await Promise.all(entryIds.map((id) => this.get(id))));
  }

  async unknownEntryFileIds(entries: readonly ClipboardEntry[]): Promise<string[]> {
    const ids = new Set<string>();
    for (const entry of entries) for (const file of entryContents(entry)) if (file.fileId) ids.add(file.fileId);
    const unknown: string[] = [];
    for (const id of ids) if (!(await this.database.get<FileStatus>("files", id))?.stored) unknown.push(id);
    return unknown;
  }

  async upsertFiles(statuses: FileStatus[]): Promise<void> {
    await this.database.batch(statuses.map((value) => ({ name: "files", id: value.fileId, value })));
  }

  async display(entryIds: string[], lightweight = true): Promise<LocalClipboardEntry[]> {
    const entries = await Promise.all(entryIds.map((id) => this.get(id)));
    return this.displayEntries(entries, lightweight);
  }

  async displayEntries(entries: readonly StoredEntry[], lightweight = true): Promise<LocalClipboardEntry[]> {
    const ids = [...new Set(entries.flatMap((entry) => entryContents(entry).map((file) => file.fileId)).filter(Boolean))];
    const available = await this.files.resolveMany(ids);
    const results: LocalClipboardEntry[] = [];
    const stored = new Set((await this.database.getMany<FileStatus>("files",ids)).filter((file) => file?.stored).map((file) => file!.fileId));
    for (const entry of entries) results.push(await summarize(entry, stored, async (fileId) => available.get(fileId), lightweight));
    return results;
  }
}
