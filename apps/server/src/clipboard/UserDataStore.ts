import { createHash } from "node:crypto";
import {
  ClipboardEntrySchema,
  FileInfoSchema,
  ImageInfoSchema,
  DeviceSchema,
  entryContents,
  type ClipboardEntry,
  type ClipboardManifestEntry,
  type Device,
  type EntryManifestQuery,
  type EntryPublishInput,
  type FileInfo,
  type ImageInfo,
} from "@cliproam/protocol";
import type Database from "better-sqlite3";
import { FileStore } from "../files/FileStore.js";
import { chunk, openDatabase, QUERY_BATCH, withTransaction } from "../sqlite.js";
import { userDatabasePath } from "../DataPaths.js";

type EntryRow = {
  id: number;
  kind: string;
  content: string;
  extra: string;
  source_device_id: string;
  created_at: string;
  pinned: number;
};

// Clipboard records and devices. Contents live in an independent pool that this
// store only references by id, so a tree is the sole record of which bytes an
// entry needs.
export class UserDataStore {
  readonly #database: Database.Database;

  constructor(userId: string, readonly files: FileStore) {
    const databasePath = userDatabasePath(userId);
    this.#database = openDatabase(databasePath);
    this.#applySchema();
  }

  #applySchema(): void {
    // Identity used to be client-generated hex strings; entries now carry a
    // server-assigned rowid plus a content hash, so a pre-hash table holds
    // rows no client can address any more and is dropped outright.
    const existing = this.#database
      .prepare("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'entry'")
      .get() as { sql: string } | undefined;
    if (existing && !existing.sql.includes("hash")) this.#database.exec("DROP TABLE entry");
    this.#database.exec(`
      CREATE TABLE IF NOT EXISTS entry (
        id INTEGER PRIMARY KEY,
        hash TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL,
        content TEXT NOT NULL,
        extra TEXT NOT NULL DEFAULT '{}',
        source_device_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        pinned INTEGER NOT NULL
      );

      CREATE TABLE IF NOT EXISTS device (
        device_id TEXT PRIMARY KEY,
        device_info TEXT NOT NULL,
        updated_at TEXT NOT NULL
      );
    `);
  }

  // Offset pagination over entry identities with optional keyword and UTC
  // date-range filters. One extra row is fetched to decide hasMore without a
  // separate count query. Full details ride POST /entries/query.
  listManifestPage(query: EntryManifestQuery, limit: number): { manifest: ClipboardManifestEntry[]; hasMore: boolean } {
    const rows = this.#database
      .prepare(`
        SELECT id
        FROM entry
        WHERE (@search IS NULL OR content LIKE @search ESCAPE '\\')
          AND (@dayStart IS NULL OR created_at BETWEEN @dayStart AND @dayEnd)
        ORDER BY id DESC
        LIMIT @limit OFFSET @offset
      `)
      .all({
        search: query.search ? `%${escapeLike(query.search)}%` : null,
        dayStart: query.dateStart ? `${query.dateStart}T00:00:00.000Z` : null,
        dayEnd: query.dateEnd ? `${query.dateEnd}T23:59:59.999Z` : null,
        limit: limit + 1,
        offset: ((query.page ?? 1) - 1) * limit,
      }) as Array<{ id: number }>;
    const hasMore = rows.length > limit;
    return { manifest: rows.slice(0, limit).map(({ id }) => ({ id: String(id) })), hasMore };
  }

  listByIds(entryIds: readonly string[]): ClipboardEntry[] {
    const entries: ClipboardEntry[] = [];
    for (const batch of chunk([...new Set(entryIds)], QUERY_BATCH)) {
      const ids = batch.map(Number).filter((id) => Number.isInteger(id));
      if (!ids.length) continue;
      const rows = this.#database
        .prepare(`
          SELECT id, kind, content, extra, source_device_id, created_at, pinned
          FROM entry
          WHERE id IN (${ids.map(() => "?").join(",")})
          ORDER BY id DESC
        `)
        .all(...ids) as Array<EntryRow>;
      for (const row of rows) {
        const entry = this.#toEntry(row);
        if (entry) entries.push(entry);
      }
    }
    return entries;
  }

  upsertDevice(device: Device): void {
    const deviceInfo = {
      name: device.name,
      platform: device.platform,
      osVersion: device.osVersion,
    };
    this.#database.prepare(`
      INSERT INTO device (device_id, device_info, updated_at)
      VALUES (?, ?, ?)
      ON CONFLICT(device_id) DO UPDATE SET device_info = excluded.device_info, updated_at = excluded.updated_at
    `).run(device.id, JSON.stringify(deviceInfo), new Date().toISOString());
  }

  listDevices(): Device[] {
    const rows = this.#database.prepare("SELECT device_id, device_info FROM device ORDER BY updated_at DESC")
      .all() as Array<{ device_id: string; device_info: string }>;
    return rows.flatMap(({ device_id, device_info }) => {
      const result = DeviceSchema.safeParse({
        ...JSON.parse(device_info),
        id: device_id,
      });
      return result.success ? [result.data] : [];
    });
  }

  // The server owns identity: it dedupes by content hash, assigns the rowid
  // and stamps arrival time. The client's id and clock are ignored, so a
  // retried publish cannot mint a second row and re-copying bumps the entry
  // back to the top.
  upsert(entry: EntryPublishInput): ClipboardEntry {
    const createdAt = new Date().toISOString();
    const extra = JSON.stringify({
      html: entry.html,
      rtf: entry.rtf,
      fileInfo: entry.fileInfo,
      imageInfo: entry.imageInfo,
    });
    const row = this.#transaction(() => {
      const row = this.#database.prepare(`
        INSERT INTO entry (
          hash, kind, content, extra, source_device_id, created_at, pinned
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(hash) DO UPDATE SET
          kind = excluded.kind,
          content = excluded.content,
          extra = excluded.extra,
          source_device_id = excluded.source_device_id,
          created_at = excluded.created_at,
          pinned = excluded.pinned
        RETURNING id, created_at
      `).get(
        entryHash(entry),
        entry.kind,
        entry.content,
        extra,
        entry.sourceDeviceId,
        createdAt,
        Number(entry.pinned),
      ) as { id: number; created_at: string };
      this.files.register(entryContents(entry));
      return row;
    });
    return {
      ...entry,
      id: String(row.id),
      createdAt: row.created_at,
      pinned: entry.pinned,
    };
  }

  // Content is shared across entries, so deletion only drops the reference.
  // Unreferenced bytes are reclaimed by collectGarbage().
  delete(entryId: string): void {
    this.#database.prepare("DELETE FROM entry WHERE id = ?").run(entryId);
  }

  // The server-wide pool owns collection. This returns this account's mark set
  // so ClipRoamStore can union it with every other account before reclaiming.
  referencedFileIds(): Set<string> {
    const referenced = new Set<string>();
    const rows = this.#database.prepare("SELECT kind, extra FROM entry").all() as Array<{ kind: string; extra: string }>;
    for (const row of rows) {
      for (const { fileId } of entryContents({ kind: row.kind, ...parseExtra(row.extra) })) referenced.add(fileId);
    }
    return referenced;
  }

  hasFileReference(entryId: string, downloadId: string): boolean {
    const row = this.#database.prepare("SELECT kind, extra FROM entry WHERE id = ?")
      .get(entryId) as { kind: string; extra: string } | undefined;
    return Boolean(row && entryContents({ kind: row.kind, ...parseExtra(row.extra) })
      .some(({ fileId }) => fileId === downloadId));
  }

  close(): void { this.#database.close(); }

  #toEntry(row: EntryRow): ClipboardEntry | undefined {
    const extra = parseExtra(row.extra);
    const result = ClipboardEntrySchema.safeParse({
      html: extra.html,
      rtf: extra.rtf,
      fileInfo: extra.fileInfo,
      imageInfo: extra.imageInfo,
      id: String(row.id),
      kind: row.kind,
      content: row.content,
      sourceDeviceId: row.source_device_id,
      createdAt: row.created_at,
      pinned: Boolean(row.pinned),
    });
    return result.success ? result.data : undefined;
  }

  #transaction<T>(work: () => T): T {
    return withTransaction(this.#database, work);
  }
}

// LIKE wildcards in user input must match literally, so they are escaped and
// the statement declares '\\' as the escape character.
function escapeLike(value: string): string {
  return value.replace(/[\\%_]/g, (character) => `\\${character}`);
}

// Content fingerprint for dedup, deliberately free of device identity: the
// same clipboard text captured anywhere collapses into one entry. Kind
// prefixes keep the three payload spaces disjoint. File entries hash their
// whole sorted content-id set — tree order must not matter — falling back to
// the summary text while background hashing is still in flight.
function entryHash(entry: {
  kind: string;
  content: string;
  fileInfo?: FileInfo;
  imageInfo?: ImageInfo;
}): string {
  const payload = entry.kind === "text"
    ? entry.content
    : entryContents(entry).map(({ fileId }) => fileId).sort().join("\n") || entry.content;
  return createHash("sha256").update(`${entry.kind}\0${payload}`).digest("hex");
}

function parseExtra(extra: string): { html?: string; rtf?: string; fileInfo?: FileInfo; imageInfo?: ImageInfo } {
  let raw: { html?: unknown; rtf?: unknown; fileInfo?: unknown; imageInfo?: unknown };
  try {
    raw = JSON.parse(extra) as typeof raw;
  } catch {
    return {};
  }
  const fileInfo = raw.fileInfo === undefined ? undefined : FileInfoSchema.safeParse(raw.fileInfo);
  const imageInfo = raw.imageInfo === undefined ? undefined : ImageInfoSchema.safeParse(raw.imageInfo);
  return {
    html: typeof raw.html === "string" ? raw.html : undefined,
    rtf: typeof raw.rtf === "string" ? raw.rtf : undefined,
    fileInfo: fileInfo?.success ? fileInfo.data : undefined,
    imageInfo: imageInfo?.success ? imageInfo.data : undefined,
  };
}
