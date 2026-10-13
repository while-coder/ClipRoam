import { entryContents, type ClipboardEntry, type ClipboardManifestEntry, type FileStatus } from "@cliproam/protocol";
import { AccountDatabase } from "./database";
import { summarize, type PendingRow, type StoredEntry } from "./content";
import { AccountFiles } from "../platform/files";
import type { LocalClipboardEntry } from "../types";

export class HistoryCache {
  readonly database: AccountDatabase;
  constructor(readonly files: AccountFiles) { this.database = files.database; }

  async get(entryId: string): Promise<StoredEntry> {
    const pending = /^p\d+$/.test(entryId) ? await this.database.get<PendingRow>("pending", entryId.slice(1)) : undefined;
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

  async pendingRows(): Promise<PendingRow[]> {
    return (await this.database.list<PendingRow>("pending")).sort((a, b) => a.seq - b.seq);
  }

  async pendingDisplay(): Promise<LocalClipboardEntry[]> {
    const rows = await this.pendingRows();
    const ids = [...new Set(rows.flatMap((row) => entryContents(row.entry).map((file) => file.fileId)).filter(Boolean))];
    const available = await this.files.resolveMany(ids);
    const entries: LocalClipboardEntry[] = [];
    for (const row of rows) entries.push({ ...await summarize(row.entry, new Set(), async (id) => available.get(id), true), pendingStatus: row.status });
    return entries;
  }

  async enqueue(entry: StoredEntry, captureId?: string, resolving = false): Promise<number> {
    const existing = captureId ? await this.database.get<number>("meta", `capture:${captureId}`) : undefined;
    if (existing !== undefined) return existing;
    const seq = await this.database.nextSequence();
    const row: PendingRow = { seq, status: "wait", entry: { ...entry, id: `p${seq}` }, captureId, resolving };
    await this.database.batch([
      { name: "pending", id: String(seq), value: row },
      ...(captureId ? [{ name: "meta" as const, id: `capture:${captureId}`, value: seq }] : []),
    ]);
    return seq;
  }

  setPendingStatus(seq: number, status: "wait" | "failed"): Promise<void> {
    return this.database.update<PendingRow>("pending", String(seq), (row) => row ? { ...row, status } : undefined);
  }
}
