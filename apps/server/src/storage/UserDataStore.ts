import { randomUUID } from "node:crypto";
import { mkdirSync, readdirSync, rmSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import {
  ClipboardEntrySchema,
  ClipboardTreeSchema,
  DeviceSchema,
  type ClipboardEntry,
  type ClipboardFile,
  type ClipboardManifestEntry,
  type ClipboardTree,
  type Device,
} from "@cliproam/protocol";
import { userDatabasePath, userFilesDirectory } from "./DataPaths.js";

// Bumping this drops and rebuilds the clipboard tables. File storage moved to
// content addressing, which no old row can be translated into.
const SCHEMA_VERSION = 2;
// SQLite allows 999 bound parameters by default, and one entry can reference
// far more files than that.
const QUERY_BATCH = 500;
const FILE_ID_PATTERN = /^[0-9a-f]{64}$/;

type EntryRow = {
  id: string;
  client_id: string;
  kind: string;
  content: string;
  extra: string;
  source_device_id: string;
  created_at: string;
  pinned: number;
};
type FileRow = { file_id: string; size: number; mime: string | null; stored: number };

export class UserDataStore {
  readonly #database: DatabaseSync;
  readonly #filesDirectory: string;

  constructor(userId: string) {
    const databasePath = userDatabasePath(userId);
    this.#filesDirectory = userFilesDirectory(userId);
    mkdirSync(dirname(databasePath), { recursive: true });
    mkdirSync(this.#filesDirectory, { recursive: true });
    this.#database = new DatabaseSync(databasePath);
    this.#database.exec("PRAGMA journal_mode = WAL;");
    this.#applySchema();
  }

  #applySchema(): void {
    const version = (this.#database.prepare("PRAGMA user_version").get() as
      { user_version: number } | undefined)?.user_version ?? 0;
    if (version !== SCHEMA_VERSION) {
      this.#database.exec(`
        DROP TABLE IF EXISTS clipboard_entries;
        DROP TABLE IF EXISTS files;
        DROP TABLE IF EXISTS upload_sessions;
      `);
      rmSync(this.#filesDirectory, { recursive: true, force: true });
      mkdirSync(this.#filesDirectory, { recursive: true });
    }
    this.#database.exec(`
      CREATE TABLE IF NOT EXISTS clipboard_entries (
        id TEXT PRIMARY KEY,
        client_id TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL,
        content TEXT NOT NULL,
        extra TEXT NOT NULL DEFAULT '{}',
        source_device_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        pinned INTEGER NOT NULL
      );
      CREATE INDEX IF NOT EXISTS clipboard_entries_created_at
        ON clipboard_entries(created_at DESC);

      -- Content pool. Rows describe bytes, never entries: 'stored' says whether
      -- the server actually holds them, 'size'/'mime' are known as soon as any
      -- entry references the content so peers can render totals before upload.
      CREATE TABLE IF NOT EXISTS files (
        file_id TEXT PRIMARY KEY,
        size INTEGER NOT NULL,
        mime TEXT,
        stored INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL
      );

      CREATE TABLE IF NOT EXISTS devices (
        device_id TEXT PRIMARY KEY,
        device_info TEXT NOT NULL,
        updated_at TEXT NOT NULL
      );
    `);
    const deviceColumns = this.#database.prepare("PRAGMA table_info(devices)")
      .all() as Array<{ name: string }>;
    if (deviceColumns.some((column) => column.name === "payload")
      && !deviceColumns.some((column) => column.name === "device_info")) {
      this.#database.exec("ALTER TABLE devices RENAME COLUMN payload TO device_info");
    }
    this.#database.exec(`PRAGMA user_version = ${SCHEMA_VERSION};`);
  }

  list(): ClipboardEntry[] {
    const rows = this.#database
      .prepare(`
        SELECT id, client_id, kind, content, extra, source_device_id, created_at, pinned
        FROM clipboard_entries
        ORDER BY created_at DESC
      `)
      .all() as Array<EntryRow>;
    return rows.flatMap((row) => {
      const entry = this.#toEntry(row);
      return entry ? [entry] : [];
    });
  }

  listByIds(entryIds: readonly string[]): ClipboardEntry[] {
    const entries: ClipboardEntry[] = [];
    for (const batch of chunk([...new Set(entryIds)], QUERY_BATCH)) {
      const rows = this.#database
        .prepare(`
          SELECT id, client_id, kind, content, extra, source_device_id, created_at, pinned
          FROM clipboard_entries
          WHERE id IN (${batch.map(() => "?").join(",")})
          ORDER BY created_at DESC
        `)
        .all(...batch) as Array<EntryRow>;
      for (const row of rows) {
        const entry = this.#toEntry(row);
        if (entry) entries.push(entry);
      }
    }
    return entries;
  }

  // The connection-time manifest only needs identity, and loading full entries
  // for it would parse every directory tree in the account.
  listManifest(): ClipboardManifestEntry[] {
    const rows = this.#database
      .prepare("SELECT id, client_id FROM clipboard_entries ORDER BY created_at DESC")
      .all() as Array<{ id: string; client_id: string }>;
    return rows.map((row) => ({ id: row.id, clientId: row.client_id }));
  }

  upsertDevice(device: Device): void {
    const deviceInfo = {
      name: device.name,
      platform: device.platform,
      osVersion: device.osVersion,
    };
    this.#database.prepare(`
      INSERT INTO devices (device_id, device_info, updated_at)
      VALUES (?, ?, ?)
      ON CONFLICT(device_id) DO UPDATE SET device_info = excluded.device_info, updated_at = excluded.updated_at
    `).run(device.id, JSON.stringify(deviceInfo), new Date().toISOString());
  }

  listDevices(): Device[] {
    const rows = this.#database.prepare("SELECT device_id, device_info FROM devices ORDER BY updated_at DESC")
      .all() as Array<{ device_id: string; device_info: string }>;
    return rows.flatMap(({ device_id, device_info }) => {
      const result = DeviceSchema.safeParse({
        ...JSON.parse(device_info),
        id: device_id,
      });
      return result.success ? [result.data] : [];
    });
  }

  upsert(entry: ClipboardEntry): ClipboardEntry {
    const clientId = entry.clientId ?? entry.id;
    const existing = this.#database
      .prepare("SELECT id FROM clipboard_entries WHERE client_id = ?")
      .get(clientId) as { id: string } | undefined;
    const storedEntry: ClipboardEntry = {
      ...entry,
      id: existing?.id ?? randomUUID(),
      clientId,
    };
    this.#transaction(() => {
      this.#database.prepare(`
        INSERT INTO clipboard_entries (
          id, client_id, kind, content, extra, source_device_id, created_at, pinned
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
          client_id = excluded.client_id,
          kind = excluded.kind,
          content = excluded.content,
          extra = excluded.extra,
          source_device_id = excluded.source_device_id,
          created_at = excluded.created_at,
          pinned = excluded.pinned
      `).run(
        storedEntry.id,
        clientId,
        storedEntry.kind,
        storedEntry.content,
        JSON.stringify({
          html: storedEntry.html,
          rtf: storedEntry.rtf,
          tree: storedEntry.tree,
        }),
        storedEntry.sourceDeviceId,
        storedEntry.createdAt,
        Number(storedEntry.pinned),
      );
      this.#registerContents(storedEntry.files);
    });
    return { ...storedEntry, files: this.#contentsOf(storedEntry.tree) };
  }

  entryIdForClientId(clientId: string): string | undefined {
    return (this.#database
      .prepare("SELECT id FROM clipboard_entries WHERE client_id = ?")
      .get(clientId) as { id: string } | undefined)?.id;
  }

  // Content is shared across entries, so deletion only drops the reference.
  // Unreferenced bytes are reclaimed by collectGarbage().
  delete(entryId: string): void {
    this.#database.prepare("DELETE FROM clipboard_entries WHERE id = ?").run(entryId);
  }

  filePath(fileId: string): string {
    if (!FILE_ID_PATTERN.test(fileId)) throw new Error("Invalid file ID for storage path");
    return join(this.#filesDirectory, fileId.slice(0, 2), fileId);
  }

  // Creates the shard directory so callers can write straight away.
  prepareFilePath(fileId: string): string {
    const path = this.filePath(fileId);
    mkdirSync(dirname(path), { recursive: true });
    return path;
  }

  storeFile(fileId: string, size: number, mime?: string): void {
    this.#database.prepare(`
      INSERT INTO files (file_id, size, mime, stored, created_at)
      VALUES (?, ?, ?, 1, ?)
      ON CONFLICT(file_id) DO UPDATE SET stored = 1, size = excluded.size
    `).run(fileId, size, mime ?? null, new Date().toISOString());
  }

  hasFile(fileId: string): boolean {
    return Boolean(this.#database
      .prepare("SELECT 1 FROM files WHERE file_id = ? AND stored = 1")
      .get(fileId));
  }

  getFile(fileId: string): { path: string; size: number } | undefined {
    const file = this.#database
      .prepare("SELECT size FROM files WHERE file_id = ? AND stored = 1")
      .get(fileId) as { size: number } | undefined;
    return file && { path: this.filePath(fileId), size: file.size };
  }

  // Mark-and-sweep over every entry's tree. Content addressing means a blob can
  // be reachable from any number of entries, so per-entry cleanup would delete
  // bytes that are still in use.
  collectGarbage(partialTtlMs: number): { removedFiles: number; removedBytes: number } {
    const referenced = new Set<string>();
    const rows = this.#database.prepare("SELECT extra FROM clipboard_entries").all() as Array<{ extra: string }>;
    for (const row of rows) {
      for (const node of parseExtra(row.extra).tree?.files ?? []) referenced.add(node.f);
    }

    let removedFiles = 0;
    let removedBytes = 0;
    this.#transaction(() => {
      const known = this.#database.prepare("SELECT file_id FROM files").all() as Array<{ file_id: string }>;
      const remove = this.#database.prepare("DELETE FROM files WHERE file_id = ?");
      for (const { file_id } of known) {
        if (!referenced.has(file_id)) remove.run(file_id);
      }
    });

    for (const bucket of readDirectorySafely(this.#filesDirectory)) {
      const bucketPath = join(this.#filesDirectory, bucket);
      for (const name of readDirectorySafely(bucketPath)) {
        const path = join(bucketPath, name);
        const partial = name.endsWith(".part");
        const fileId = partial ? name.slice(0, -".part".length) : name;
        // A .part belongs to an upload in flight; only age retires it.
        if (partial ? !isExpired(path, partialTtlMs) : referenced.has(fileId)) continue;
        removedBytes += sizeOf(path);
        rmSync(path, { force: true });
        removedFiles += 1;
      }
      if (readDirectorySafely(bucketPath).length === 0) rmSync(bucketPath, { recursive: true, force: true });
    }
    return { removedFiles, removedBytes };
  }

  close(): void { this.#database.close(); }

  #toEntry(row: EntryRow): ClipboardEntry | undefined {
    const extra = parseExtra(row.extra);
    const result = ClipboardEntrySchema.safeParse({
      html: extra.html,
      rtf: extra.rtf,
      tree: extra.tree,
      id: row.id,
      clientId: row.client_id,
      kind: row.kind,
      content: row.content,
      files: this.#contentsOf(extra.tree),
      sourceDeviceId: row.source_device_id,
      createdAt: row.created_at,
      pinned: Boolean(row.pinned),
    });
    return result.success ? result.data : undefined;
  }

  // The tree is the only record of which content an entry uses; the file table
  // supplies size, mime and availability for each distinct reference.
  #contentsOf(tree: ClipboardTree | undefined): ClipboardFile[] {
    const fileIds = [...new Set((tree?.files ?? []).map((node) => node.f))];
    if (fileIds.length === 0) return [];
    const known = new Map<string, FileRow>();
    for (const batch of chunk(fileIds, QUERY_BATCH)) {
      const rows = this.#database.prepare(`
        SELECT file_id, size, mime, stored FROM files
        WHERE file_id IN (${batch.map(() => "?").join(",")})
      `).all(...batch) as Array<FileRow>;
      for (const file of rows) known.set(file.file_id, file);
    }
    return fileIds.map((fileId) => {
      const file = known.get(fileId);
      return {
        fileId,
        size: file?.size ?? 0,
        mime: file?.mime ?? undefined,
        available: Boolean(file?.stored),
      };
    });
  }

  #registerContents(files: readonly ClipboardFile[]): void {
    const insert = this.#database.prepare(`
      INSERT INTO files (file_id, size, mime, stored, created_at)
      VALUES (?, ?, ?, 0, ?)
      ON CONFLICT(file_id) DO NOTHING
    `);
    const now = new Date().toISOString();
    for (const file of files) {
      if (FILE_ID_PATTERN.test(file.fileId)) insert.run(file.fileId, file.size, file.mime ?? null, now);
    }
  }

  #transaction(work: () => void): void {
    this.#database.exec("BEGIN");
    try {
      work();
      this.#database.exec("COMMIT");
    } catch (error) {
      this.#database.exec("ROLLBACK");
      throw error;
    }
  }
}

function parseExtra(extra: string): { html?: string; rtf?: string; tree?: ClipboardTree } {
  let raw: { html?: unknown; rtf?: unknown; tree?: unknown };
  try {
    raw = JSON.parse(extra) as typeof raw;
  } catch {
    return {};
  }
  const tree = raw.tree === undefined ? undefined : ClipboardTreeSchema.safeParse(raw.tree);
  return {
    html: typeof raw.html === "string" ? raw.html : undefined,
    rtf: typeof raw.rtf === "string" ? raw.rtf : undefined,
    tree: tree?.success ? tree.data : undefined,
  };
}

function chunk<T>(items: readonly T[], size: number): T[][] {
  const batches: T[][] = [];
  for (let index = 0; index < items.length; index += size) {
    batches.push(items.slice(index, index + size));
  }
  return batches;
}

function readDirectorySafely(path: string): string[] {
  try {
    return readdirSync(path);
  } catch {
    return [];
  }
}

function sizeOf(path: string): number {
  try {
    return statSync(path).size;
  } catch {
    return 0;
  }
}

function isExpired(path: string, ttlMs: number): boolean {
  if (ttlMs === 0) return true;
  try {
    return Date.now() - statSync(path).mtimeMs > ttlMs;
  } catch {
    return false;
  }
}
