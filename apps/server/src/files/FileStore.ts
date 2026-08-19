import { mkdirSync, readdirSync, rmSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import type { ClipboardFile } from "@cliproam/protocol";
import type Database from "better-sqlite3";
import { chunk, QUERY_BATCH, withTransaction } from "../sqlite.js";

const FILE_ID_PATTERN = /^[0-9a-f]{64}$/;
const PARTIAL_SUFFIX = ".part";
const SCHEMA_VERSION = 2;

type FileRow = { file_id: string; size: number; stored: number };

// The content pool: bytes addressed by `sha256(content)`, with no knowledge of
// clipboard entries. Nothing here records who references a content, so the same
// bytes are stored once no matter how many entries or paths point at them.
// Reclaiming is therefore driven from the outside — see `sweep`.
export class FileStore {
  constructor(
    private readonly database: Database.Database,
    private readonly directory: string,
  ) {
    mkdirSync(this.directory, { recursive: true });
  }

  applySchema(): void {
    const version = (this.database.prepare("PRAGMA user_version").get() as
      { user_version: number } | undefined)?.user_version ?? 0;
    if (version !== SCHEMA_VERSION) this.reset();
    this.database.exec(`
      -- Rows describe bytes, never entries: 'stored' says whether the server
      -- actually holds them, while size is known as soon as an entry refers to
      -- the content so peers can render totals before upload.
      CREATE TABLE IF NOT EXISTS files (
        file_id TEXT PRIMARY KEY,
        size INTEGER NOT NULL,
        stored INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL
      );
      PRAGMA user_version = ${SCHEMA_VERSION};
    `);
  }

  // Content addressing has no migration path from older layouts, so a schema
  // change discards the pool along with the tables that referenced it.
  reset(): void {
    this.database.exec(`
      DROP TABLE IF EXISTS files;
      DROP TABLE IF EXISTS upload_sessions;
    `);
    rmSync(this.directory, { recursive: true, force: true });
    mkdirSync(this.directory, { recursive: true });
  }

  path(fileId: string): string {
    if (!FILE_ID_PATTERN.test(fileId)) throw new Error("Invalid file ID for storage path");
    return join(this.directory, fileId.slice(0, 2), fileId);
  }

  // Creates the shard directory so callers can write straight away.
  preparePath(fileId: string): string {
    const path = this.path(fileId);
    mkdirSync(dirname(path), { recursive: true });
    return path;
  }

  store(fileId: string, size: number): void {
    this.database.prepare(`
      INSERT INTO files (file_id, size, stored, created_at)
      VALUES (?, ?, 1, ?)
      ON CONFLICT(file_id) DO UPDATE SET stored = 1, size = excluded.size
    `).run(fileId, size, new Date().toISOString());
  }

  has(fileId: string): boolean {
    return Boolean(this.database
      .prepare("SELECT 1 FROM files WHERE file_id = ? AND stored = 1")
      .get(fileId));
  }

  get(fileId: string): { path: string; size: number } | undefined {
    const file = this.database
      .prepare("SELECT size FROM files WHERE file_id = ? AND stored = 1")
      .get(fileId) as { size: number } | undefined;
    return file && { path: this.path(fileId), size: file.size };
  }

  // Registers contents an entry refers to but the server may not hold yet, so
  // that peers can see sizes before the upload happens.
  register(files: readonly ClipboardFile[]): void {
    const insert = this.database.prepare(`
      INSERT INTO files (file_id, size, stored, created_at)
      VALUES (?, ?, 0, ?)
      ON CONFLICT(file_id) DO NOTHING
    `);
    const now = new Date().toISOString();
    for (const file of files) {
      if (FILE_ID_PATTERN.test(file.fileId)) insert.run(file.fileId, file.size, now);
    }
  }

  // Fills in size and availability for a list of content ids.
  describe(fileIds: readonly string[]): ClipboardFile[] {
    if (fileIds.length === 0) return [];
    const known = new Map<string, FileRow>();
    for (const batch of chunk(fileIds, QUERY_BATCH)) {
      const rows = this.database.prepare(`
        SELECT file_id, size, stored FROM files
        WHERE file_id IN (${batch.map(() => "?").join(",")})
      `).all(...batch) as Array<FileRow>;
      for (const file of rows) known.set(file.file_id, file);
    }
    return fileIds.map((fileId) => {
      const file = known.get(fileId);
      return {
        fileId,
        size: file?.size ?? 0,
        available: Boolean(file?.stored),
      };
    });
  }

  // The sweep half of mark-and-sweep: the caller supplies every content id still
  // reachable from a clipboard entry, because only the entries know that.
  sweep(referenced: ReadonlySet<string>, partialTtlMs: number): { removedFiles: number; removedBytes: number } {
    withTransaction(this.database, () => {
      const known = this.database.prepare("SELECT file_id FROM files").all() as Array<{ file_id: string }>;
      const remove = this.database.prepare("DELETE FROM files WHERE file_id = ?");
      for (const { file_id } of known) {
        if (!referenced.has(file_id)) remove.run(file_id);
      }
    });

    // Disk removal stays outside the transaction: it is slow, and a crash
    // halfway through only leaves unreferenced bytes for the next sweep.
    let removedFiles = 0;
    let removedBytes = 0;
    for (const bucket of readDirectorySafely(this.directory)) {
      const bucketPath = join(this.directory, bucket);
      for (const name of readDirectorySafely(bucketPath)) {
        const path = join(bucketPath, name);
        const partial = name.endsWith(PARTIAL_SUFFIX);
        const fileId = partial ? name.slice(0, -PARTIAL_SUFFIX.length) : name;
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
