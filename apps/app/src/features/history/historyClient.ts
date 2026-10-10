import { ENTRY_QUERY_BATCH, EntryManifestResponseSchema, EntryQueryResponseSchema, FileQueryResponseSchema, type ClipboardEntry, type ClipboardManifestEntry, type FileQueryRequest, type FileStatus } from "@cliproam/protocol";
import type { AccountSession } from "../sync/accountSession";
import type { SyncRequester } from "../sync/syncHttp";
import { PAGE_SIZE } from "../../utils/constants";
import type { EntriesManifestFilter, EntriesManifestPage, LocalClipboardEntry } from "../../types";
const ENTRY_HTTP_TIMEOUT_MS = 30_000;

/** Server paging and versioned detail backfill; never reads the pending queue. */
export class HistoryClient {
  constructor(private readonly http: SyncRequester, private readonly session: AccountSession) {}
  // Splits a long id list into fixed-size batches, collecting per-batch results.
  async #queryBatched<T>(ids: readonly string[], run: (batch: string[]) => Promise<T[]>): Promise<T[]> {
    const results: T[] = [];
    for (let index = 0; index < ids.length; index += ENTRY_QUERY_BATCH) {
      results.push(...await run(ids.slice(index, index + ENTRY_QUERY_BATCH)));
    }
    return results;
  }

  #jsonInit(request: unknown, timeoutMs: number): RequestInit {
    return {
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request),
      signal: AbortSignal.timeout(timeoutMs),
    };
  }

  /** One server page defines both the visible identities and the total. */
  async fetchHistoryPage(
    filter: EntriesManifestFilter,
  ): Promise<EntriesManifestPage> {
    const params = new URLSearchParams({ page: String(filter.page ?? 1), pageSize: String(PAGE_SIZE) });
    if (filter.query?.trim()) params.set("search", filter.query.trim());
    if (filter.kind && filter.kind !== "all") params.set("kind", filter.kind);
    if (filter.start !== undefined) params.set("dateStart", new Date(filter.start).toISOString());
    if (filter.end !== undefined) params.set("dateEnd", new Date(filter.end).toISOString());
    for (const id of filter.deviceIds ?? []) params.append("deviceIds", id);
    const result = await this.http.request(
      "GET", `/entries/manifest?${params}`,
      { signal: AbortSignal.timeout(ENTRY_HTTP_TIMEOUT_MS) },
      EntryManifestResponseSchema, "服务器返回了不兼容的历史列表响应",
    );
    const entries = await this.fetchHistoryInfo(result!.manifest);
    if (this.session.signal.aborted) return { total: 0, entries: [] };
    return { total: result!.total, entries };
  }

  /** Only detail backfill writes server entry/file information into the cache. */
  async fetchHistoryInfo(
    manifest: ClipboardManifestEntry[],
  ): Promise<LocalClipboardEntry[]> {
    const entryIds = manifest.map((entry) => entry.id);
    if (!entryIds.length) return [];
    try {
      const missing = await this.session.invoke<string[]>("find_stale_entry_ids", { manifest });
      if (missing.length) {
        const entries = await this.#fetchEntries(missing);
        await this.session.invoke("upsert_server_entries", { entries });
      }
      // Only this page's details supply file identities; unknown/unstored files
      // are re-queried before computing the list's cached summaries.
      const fileIds = await this.session.invoke<string[]>("find_unknown_file_ids", { entryIds });
      if (fileIds.length) {
        const statuses = await this.#fetchFiles(fileIds);
        await this.session.invoke("upsert_server_files", { statuses });
      }
      return await this.session.invoke<LocalClipboardEntry[]>("get_cached_entries_for_display", { entryIds });
    } catch (error) {
      if (this.session.signal.aborted) return [];
      throw error;
    }
  }

  async #fetchEntries(entryIds: readonly string[]): Promise<ClipboardEntry[]> {
    return this.#queryBatched(entryIds, async (batch) => {
      const result = await this.http.request(
        "POST", "/entries/query",
        this.#jsonInit({ entryIds: batch }, ENTRY_HTTP_TIMEOUT_MS),
        EntryQueryResponseSchema, "服务器返回了不兼容的历史记录响应",
      );
      return result!.entries;
    });
  }

  // Pool availability for a batch of content ids. This replaces the per-entry
  // `missing` list the protocol dropped: the client asks once per upsert batch
  // which contents the server already holds, so locally stored availability
  // marks stay truthful without the server restamping every entry read.
  async #fetchFiles(fileIds: readonly string[]): Promise<FileStatus[]> {
    return this.#queryBatched(fileIds, (batch) => this.#fetchFileStatusBatch(batch));
  }

  async #fetchFileStatusBatch(fileIds: readonly string[]): Promise<FileStatus[]> {
    const request: FileQueryRequest = { fileIds: [...fileIds] };
    const queried = await this.http.request(
      "POST",
      "/files/query",
      this.#jsonInit(request, ENTRY_HTTP_TIMEOUT_MS),
      FileQueryResponseSchema,
      "服务器返回了不兼容的文件状态响应",
    );
    return queried!.files;
  }

  async getEntry(entryId: string): Promise<ClipboardEntry> {
    return this.session.invoke<ClipboardEntry>("get_entry", { entryId });
  }

  // A 404 is not a failure: another device may have deleted the entry first,
  // and the outcome every device converges on is the same.
  async delete(entryId: string): Promise<void> {
    await this.http.request(
      "DELETE",
      `/entries/${encodeURIComponent(entryId)}`,
      { signal: AbortSignal.timeout(ENTRY_HTTP_TIMEOUT_MS) },
      null,
      "",
      true,
    );
    await this.session.invoke("remove_server_entry", { entryId });
  }

}
