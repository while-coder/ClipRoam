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
    this.#database.exec(`
      CREATE TABLE IF NOT EXISTS entries (
        id INTEGER PRIMARY KEY,
        hash TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL,
        content TEXT NOT NULL,
        extra TEXT NOT NULL DEFAULT '{}',
        source_device_id TEXT NOT NULL,
        created_at TEXT NOT NULL
      );

      CREATE INDEX IF NOT EXISTS entries_created_at ON entries (created_at DESC);

      CREATE TABLE IF NOT EXISTS devices (
        device_id TEXT PRIMARY KEY,
        device_info TEXT NOT NULL,
        updated_at TEXT NOT NULL
      );
    `);
  }

  // Offset pagination over entry identities with optional keyword, UTC
  // date-range and kind filters. The total covers the same filters, so
  // a client can render "page x of y" and detect the last page from a filtered
  // result set. Full details ride POST /entries/query. Recency order follows
  // `created_at`: the rowid no longer reflects it, since a re-copy refreshes
  // the timestamp while keeping its original rowid.
  listManifestPage(query: EntryManifestQuery, limit: number): { manifest: ClipboardManifestEntry[]; total: number } {
    // Shared between the page read and the total count so the two can never
    // disagree about what a "matching row" is.
    const where = `
      WHERE (@search IS NULL OR content LIKE @search ESCAPE '\\')
        AND (@dateStart IS NULL OR created_at BETWEEN @dateStart AND @dateEnd)
        AND (@kind IS NULL OR kind = @kind)
    `;
    const filters = {
      search: query.search ? `%${escapeLike(query.search)}%` : null,
      dateStart: query.dateStart ? normalizeDateBound(query.dateStart, "start") : null,
      dateEnd: query.dateEnd ? normalizeDateBound(query.dateEnd, "end") : null,
      kind: query.kind ?? null,
    };
    const rows = this.#database
      .prepare(`
        SELECT id
        FROM entries
        ${where}
        ORDER BY created_at DESC
        LIMIT @limit OFFSET @offset
      `)
      .all({ ...filters, limit, offset: ((query.page ?? 1) - 1) * limit }) as Array<{ id: number }>;
    const { count } = this.#database
      .prepare(`SELECT COUNT(*) AS count FROM entries ${where}`)
      .get(filters) as { count: number };
    return {
      manifest: rows.map(({ id }) => ({ id: String(id) })),
      total: count,
    };
  }

  listByIds(entryIds: readonly string[]): ClipboardEntry[] {
    const entries: ClipboardEntry[] = [];
    for (const batch of chunk([...new Set(entryIds)], QUERY_BATCH)) {
      const ids = batch.map(Number).filter((id) => Number.isInteger(id));
      if (!ids.length) continue;
      const rows = this.#database
        .prepare(`
          SELECT id, kind, content, extra, source_device_id, created_at
          FROM entries
          WHERE id IN (${ids.map(() => "?").join(",")})
          ORDER BY created_at DESC
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
      INSERT INTO devices (device_id, device_info, updated_at)
      VALUES (?, ?, ?)
      ON CONFLICT(device_id) DO UPDATE SET device_info = excluded.device_info, updated_at = excluded.updated_at
    `).run(device.id, JSON.stringify(deviceInfo), new Date().toISOString());
  }

  listDevices(): Device[] {
    return this.#listDeviceRows().map(({ device }) => device);
  }

  // Admin view: the row's updated_at doubles as the last time the device
  // signed in or re-registered.
  listDevicesWithLastSeen(): Array<Device & { lastSeenAt: string }> {
    return this.#listDeviceRows().map(({ device, updatedAt }) => ({ ...device, lastSeenAt: updatedAt }));
  }

  #listDeviceRows(): Array<{ device: Device; updatedAt: string }> {
    const rows = this.#database.prepare("SELECT device_id, device_info, updated_at FROM devices ORDER BY updated_at DESC")
      .all() as Array<{ device_id: string; device_info: string; updated_at: string }>;
    return rows.flatMap(({ device_id, device_info, updated_at }) => {
      const result = DeviceSchema.safeParse({
        ...JSON.parse(device_info),
        id: device_id,
      });
      return result.success ? [{ device: result.data, updatedAt: updated_at }] : [];
    });
  }

  deleteDevice(deviceId: string): boolean {
    return this.#database.prepare("DELETE FROM devices WHERE device_id = ?").run(deviceId).changes > 0;
  }

  // The server owns identity: it dedupes by content hash, assigns the rowid
  // and stamps arrival time. The client's id and clock are ignored, so a
  // retried publish cannot mint a second row. A hash conflict means the same
  // content was re-copied: only `created_at` is refreshed so the entry moves
  // back to the top, and no other stored field is ever rewritten.
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
        INSERT INTO entries (
          hash, kind, content, extra, source_device_id, created_at
        ) VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(hash) DO UPDATE SET created_at = excluded.created_at
        RETURNING id, created_at
      `).get(
        entryHash(entry),
        entry.kind,
        entry.content,
        extra,
        entry.sourceDeviceId,
        createdAt,
      ) as { id: number; created_at: string };
      this.files.register(entryContents(entry));
      return row;
    });
    return {
      ...entry,
      id: String(row.id),
      createdAt: row.created_at,
    };
  }

  // Enforces the account-wide history cap: entries beyond the newest
  // `maxEntries` are dropped and their ids returned so the route layer can
  // broadcast clipboard.deleted. Like delete(), only references go away —
  // collectGarbage() reclaims the bytes later.
  prune(maxEntries: number): string[] {
    if (maxEntries <= 0) return [];
    const rows = this.#database.prepare(`
      DELETE FROM entries
      WHERE id NOT IN (SELECT id FROM entries ORDER BY created_at DESC LIMIT ?)
      RETURNING id
    `).all(maxEntries) as Array<{ id: number }>;
    return rows.map(({ id }) => String(id));
  }

  // Content is shared across entries, so deletion only drops the reference.
  // Unreferenced bytes are reclaimed by collectGarbage().
  delete(entryId: string): void {
    this.#database.prepare("DELETE FROM entries WHERE id = ?").run(entryId);
  }

  // The server-wide pool owns collection. This returns this account's mark set
  // so ClipRoamStore can union it with every other account before reclaiming.
  referencedFileIds(): Set<string> {
    const referenced = new Set<string>();
    const rows = this.#database.prepare("SELECT kind, extra FROM entries").all() as Array<{ kind: string; extra: string }>;
    for (const row of rows) {
      for (const { fileId } of entryContents({ kind: row.kind, ...parseExtra(row.extra) })) referenced.add(fileId);
    }
    return referenced;
  }

  hasFileReference(entryId: string, downloadId: string): boolean {
    const row = this.#database.prepare("SELECT kind, extra FROM entries WHERE id = ?")
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

// Range bounds compare against `created_at` (stored via `toISOString()`, i.e.
// UTC with milliseconds), so they are normalized into the same shape for the
// string comparison to be correct. A bare date expands to the whole day;
// seconds without milliseconds gain `.000` so the bound stays within the
// stored format's precision.
function normalizeDateBound(value: string, bound: "start" | "end"): string {
  if (/^\d{4}-\d{2}-\d{2}$/.test(value)) {
    return bound === "start" ? `${value}T00:00:00.000Z` : `${value}T23:59:59.999Z`;
  }
  if (/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/.test(value)) return `${value.slice(0, -1)}.000Z`;
  return value;
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
