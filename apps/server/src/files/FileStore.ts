import { mkdirSync, readdirSync, rmSync, statSync, type Stats } from "node:fs";
import { dirname, join } from "node:path";
import type Database from "better-sqlite3";
import { filesDatabasePath, filesDirectory } from "../DataPaths.js";
import { chunk, openDatabase, QUERY_BATCH, withTransaction } from "../sqlite.js";

const FILE_ID_PATTERN = /^[0-9a-f]{64}$/;
const PARTIAL_SUFFIX = ".part";
// Disk removals between event-loop yields during reclaim: keeps one GC sweep
// from monopolising the loop while still finishing a large pool quickly.
const GC_YIELD_BATCH = 200;

type FileRow = { file_id: string; size: number; stored: number };

// A pool answer for one content id: the size the first registering entry
// reported, and whether the bytes are actually on disk.
type FileStatus = { fileId: string; size: number; stored: boolean };

// The admin listing shape: a pool row with its registration timestamp.
export type FileRecord = { fileId: string; size: number; stored: boolean; createdAt: string };

// The content pool: bytes addressed by `sha256(content)`, with no knowledge of
// clipboard entries. Nothing here records who references a content, so the same
// bytes are stored once no matter how many entries or paths point at them.
// Reclaiming is therefore driven from the outside — see `reclaimUnreferenced`.
export class FileStore {
  private readonly database: Database.Database;
  private readonly directory: string;

  constructor() {
    this.database = openDatabase(filesDatabasePath);
    this.directory = filesDirectory;
    mkdirSync(this.directory, { recursive: true });
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
      -- The upload ledger: one row per content being received, keyed by the
      -- content id itself rather than a session, so any device can pick an
      -- upload up where another left it. Bitmap bit i = chunk i is on disk.
      CREATE TABLE IF NOT EXISTS upload_parts (
        file_id TEXT PRIMARY KEY,
        size INTEGER NOT NULL,
        chunk_count INTEGER NOT NULL,
        bitmap BLOB NOT NULL,
        updated_at TEXT NOT NULL
      );
    `);
  }

  close(): void { this.database.close(); }

  private path(fileId: string): string {
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
    return this.get(fileId) !== undefined;
  }

  get(fileId: string): { path: string; size: number } | undefined {
    const file = this.database
      .prepare("SELECT size FROM files WHERE file_id = ? AND stored = 1")
      .get(fileId) as { size: number } | undefined;
    return file && { path: this.path(fileId), size: file.size };
  }

  // Locates the preallocated upload buffer beside its eventual resting place so
  // promoting a finished upload is a same-directory rename. The shard directory
  // is created by `preparePath` (called by the upload's `begin`), not here — a
  // chunk write must not re-mkdir an existing directory per request.
  partialPath(fileId: string): string {
    return `${this.path(fileId)}.part`;
  }

  uploadLedger(fileId: string): { size: number; chunkCount: number; bitmap: Buffer } | undefined {
    const row = this.database
      .prepare("SELECT size, chunk_count, bitmap FROM upload_parts WHERE file_id = ?")
      .get(fileId) as { size: number; chunk_count: number; bitmap: Buffer } | undefined;
    return row && { size: row.size, chunkCount: row.chunk_count, bitmap: row.bitmap };
  }

  beginUploadLedger(fileId: string, size: number, chunkCount: number): void {
    this.database.prepare(`
      INSERT INTO upload_parts (file_id, size, chunk_count, bitmap, updated_at)
      VALUES (?, ?, ?, ?, ?)
      ON CONFLICT(file_id) DO UPDATE SET
        size = excluded.size, chunk_count = excluded.chunk_count,
        bitmap = excluded.bitmap, updated_at = excluded.updated_at
    `).run(fileId, size, chunkCount, zeroBitmap(chunkCount), new Date().toISOString());
  }

  // Marks one chunk written and reports whether the ledger is now full. Returns
  // undefined when reclamation removed the row while the request was in flight.
  markChunkWritten(fileId: string, index: number): { bitmap: Buffer; full: boolean } | undefined {
    return withTransaction(this.database, () => {
      const ledger = this.uploadLedger(fileId);
      if (!ledger) return undefined;
      const bitmap = Buffer.from(ledger.bitmap);
      bitmap[index >> 3] |= 1 << (index & 7);
      this.database
        .prepare("UPDATE upload_parts SET bitmap = ?, updated_at = ? WHERE file_id = ?")
        .run(bitmap, new Date().toISOString(), fileId);
      return { bitmap, full: isBitmapFull(bitmap, ledger.chunkCount) };
    });
  }

  removeUploadLedger(fileId: string): void {
    this.database.prepare("DELETE FROM upload_parts WHERE file_id = ?").run(fileId);
  }

  // Registers contents an entry refers to but the server may not hold yet, so
  // that peers can see sizes before the upload happens.
  register(contents: ReadonlyArray<{ fileId: string; size: number }>): void {
    const insert = this.database.prepare(`
      INSERT INTO files (file_id, size, stored, created_at)
      VALUES (?, ?, 0, ?)
      ON CONFLICT(file_id) DO NOTHING
    `);
    const now = new Date().toISOString();
    for (const content of contents) {
      if (FILE_ID_PATTERN.test(content.fileId)) insert.run(content.fileId, content.size, now);
    }
  }

  // Fills in size and availability for a list of content ids.
  describe(fileIds: readonly string[]): FileStatus[] {
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
        stored: Boolean(file?.stored),
      } satisfies FileStatus;
    });
  }

  // Admin view: newest registrations first, id-prefix search, bounded page so
  // a huge pool cannot flood the response.
  listFiles(search: string | undefined, limit: number): { files: FileRecord[]; total: number } {
    const pattern = search?.trim() ? `${escapeLike(search.trim().toLowerCase())}%` : null;
    const where = pattern ? "WHERE file_id LIKE ? ESCAPE '\\'" : "";
    const { total } = this.database
      .prepare(`SELECT COUNT(*) AS total FROM files ${where}`)
      .get(...(pattern ? [pattern] : [])) as { total: number };
    const rows = this.database
      .prepare(`SELECT file_id, size, stored, created_at FROM files ${where} ORDER BY created_at DESC LIMIT ?`)
      .all(...(pattern ? [pattern] : []), limit) as Array<FileRow & { created_at: string }>;
    return {
      files: rows.map(({ file_id, size, stored, created_at }) => ({
        fileId: file_id,
        size,
        stored: Boolean(stored),
        createdAt: created_at,
      })),
      total,
    };
  }

  stats(): { count: number; storedCount: number; storedBytes: number } {
    return this.database.prepare(`
      SELECT COUNT(*) AS count,
        COALESCE(SUM(stored), 0) AS storedCount,
        COALESCE(SUM(CASE WHEN stored = 1 THEN size ELSE 0 END), 0) AS storedBytes
      FROM files
    `).get() as { count: number; storedCount: number; storedBytes: number };
  }

  // Admin removal: the bytes, the registration row and any idle upload ledger
  // go together. Entries still referencing the content lose it on read —
  // the caller owns that warning in the UI.
  deleteFile(fileId: string): boolean {
    if (!FILE_ID_PATTERN.test(fileId)) return false;
    const removed = this.database.prepare("DELETE FROM files WHERE file_id = ?").run(fileId).changes > 0;
    if (removed) {
      rmSync(this.path(fileId), { force: true });
      rmSync(this.partialPath(fileId), { force: true });
      this.removeUploadLedger(fileId);
    }
    return removed;
  }

  // Deletes every content the caller does not claim as still reachable from a
  // clipboard entry, plus .part uploads idle past `partialTtlMs`. The caller
  // supplies the reachable set because only the entries know it. Async and
  // yielding: a pool of a few thousand files would otherwise block the event
  // loop — every request, WebSocket push and heartbeat — for the whole walk.
  async reclaimUnreferenced(referenced: ReadonlySet<string>, partialTtlMs: number): Promise<{ removedFiles: number; removedBytes: number }> {
    withTransaction(this.database, () => {
      const known = this.database.prepare("SELECT file_id FROM files").all() as Array<{ file_id: string }>;
      const remove = this.database.prepare("DELETE FROM files WHERE file_id = ?");
      for (const { file_id } of known) {
        if (!referenced.has(file_id)) remove.run(file_id);
      }
    });

    // Disk removal stays outside the transaction: it is slow, and a crash
    // halfway through only leaves unreferenced bytes for the next run.
    let removedFiles = 0;
    let removedBytes = 0;
    const removeLedger = this.database.prepare("DELETE FROM upload_parts WHERE file_id = ?");
    // The `referenced` snapshot was taken before this walk started: content
    // promoted in between has a registration row but no snapshot entry, so
    // final files are kept on a live table check, not the stale snapshot.
    const isRegistered = this.database.prepare("SELECT 1 FROM files WHERE file_id = ?");
    let processed = 0;
    for (const bucket of readDirectorySafely(this.directory)) {
      const bucketPath = join(this.directory, bucket);
      const names = readDirectorySafely(bucketPath);
      let remaining = names.length;
      for (const name of names) {
        const path = join(bucketPath, name);
        const partial = name.endsWith(PARTIAL_SUFFIX);
        const fileId = partial ? name.slice(0, -PARTIAL_SUFFIX.length) : name;
        // One stat answers both questions — whether a .part has aged out and
        // how many bytes retiring it reclaims. A stat failure keeps the file
        // for the next run to look at.
        const stats = statsOf(path);
        if (!stats) continue;
        const keep = partial
          ? !isExpired(stats.mtimeMs, partialTtlMs)
          : referenced.has(fileId) || isRegistered.get(fileId) !== undefined;
        if (keep) continue;
        removedBytes += stats.size;
        rmSync(path, { force: true });
        // The ledger must not outlive the bytes it describes, or a later `begin`
        // would report chunks that no longer exist.
        if (partial) removeLedger.run(fileId);
        removedFiles += 1;
        remaining -= 1;
        processed += 1;
        if (processed % GC_YIELD_BATCH === 0) {
          await new Promise((resolve) => setImmediate(resolve));
        }
      }
      if (remaining === 0) rmSync(bucketPath, { recursive: true, force: true });
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

// Bit `i` of a bitmap lives in byte `i >> 3`, counting from the least
// significant bit, and the tail bits past `chunkCount` stay zero so a ledger is
// byte-for-byte reproducible.
export function zeroBitmap(chunkCount: number): Buffer {
  return Buffer.alloc(Math.ceil(chunkCount / 8));
}

export function isBitmapFull(bitmap: Buffer, chunkCount: number): boolean {
  const fullBytes = chunkCount >> 3;
  for (let index = 0; index < fullBytes; index++) {
    if (bitmap[index] !== 0xff) return false;
  }
  const tailBits = chunkCount & 7;
  return tailBits === 0 || bitmap[fullBytes] === (1 << tailBits) - 1;
}

export function countWrittenChunks(bitmap: Buffer): number {
  let count = 0;
  for (const byte of bitmap) {
    // Kernighan's trick: clearing the lowest set bit once per set bit.
    let value = byte;
    while (value) {
      value &= value - 1;
      count += 1;
    }
  }
  return count;
}

function statsOf(path: string): Stats | undefined {
  try {
    return statSync(path);
  } catch {
    return undefined;
  }
}

function escapeLike(value: string): string {
  return value.replace(/[\\%_]/g, "\\$&");
}

function isExpired(mtimeMs: number, ttlMs: number): boolean {
  if (ttlMs === 0) return true;
  return Date.now() - mtimeMs > ttlMs;
}
