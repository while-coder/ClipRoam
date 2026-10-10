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
import { chunk, escapeLike, openDatabase, placeholders, QUERY_BATCH, withTransaction } from "../sqlite.js";
import { userDatabasePath } from "../DataPaths.js";

type EntryRow = {
  id: number;
  version: number;
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
        version INTEGER NOT NULL DEFAULT 1,
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
    const columns = this.#database.pragma("table_info(entries)") as Array<{ name: string }>;
    if (!columns.some(({ name }) => name === "version")) {
      this.#database.exec("ALTER TABLE entries ADD COLUMN version INTEGER NOT NULL DEFAULT 1");
    }
    if (this.#database.pragma("user_version", { simple: true }) === 0) {
      this.#transaction(() => {
        const rows = this.#database.prepare("SELECT id, kind, content, extra FROM entries WHERE kind = 'files'").all() as EntryRow[];
        const update = this.#database.prepare("UPDATE entries SET hash = ? WHERE id = ?");
        for (const row of rows) update.run(entryHash({ kind: row.kind, content: row.content, ...parseExtra(row.extra) }), row.id);
        this.#database.pragma("user_version = 1");
      });
    }
  }

  // Offset pagination over entry identities with optional keyword, UTC
  // date-range, kind and source-device filters. The total covers the same
  // filters, so a client can render "page x of y" and detect the last page
  // from a filtered result set. Full details ride POST /entries/query.
  // Recency order follows `created_at`: the rowid no longer reflects it, since
  // a re-copy refreshes the timestamp while keeping its original rowid.
  listManifestPage(query: EntryManifestQuery, limit: number): { manifest: ClipboardManifestEntry[]; total: number } {
    // The device filter matches entries whose source is one of the selected
    // devices. An absent filter drops the clause entirely.
    const selectedDeviceIds = [...new Set(query.deviceIds ?? [])];
    const deviceParams = Object.fromEntries(selectedDeviceIds.map((id, index) => [`device${index}`, id]));
    const deviceClause = selectedDeviceIds.length
      ? `source_device_id IN (${Object.keys(deviceParams).map((name) => `@${name}`).join(", ")})`
      : "";
    // Shared between the page read and the total count so the two can never
    // disagree about what a "matching row" is.
    const where = `
      WHERE (@search IS NULL OR content LIKE @search ESCAPE '\\')
        AND (@dateStart IS NULL OR created_at BETWEEN @dateStart AND @dateEnd)
        AND (@kind IS NULL OR kind = @kind)
        ${deviceClause ? `AND (${deviceClause})` : ""}
    `;
    const filters = {
      search: query.search ? `%${escapeLike(query.search)}%` : null,
      dateStart: query.dateStart ? normalizeDateBound(query.dateStart, "start") : null,
      dateEnd: query.dateEnd ? normalizeDateBound(query.dateEnd, "end") : null,
      kind: query.kind ?? null,
      ...deviceParams,
    };
    const rows = this.#database
      .prepare(`
        SELECT id, version
        FROM entries
        ${where}
        ORDER BY created_at DESC
        LIMIT @limit OFFSET @offset
      `)
      .all({ ...filters, limit, offset: ((query.page ?? 1) - 1) * limit }) as Array<{ id: number; version: number }>;
    const { count } = this.#database
      .prepare(`SELECT COUNT(*) AS count FROM entries ${where}`)
      .get(filters) as { count: number };
    return {
      manifest: rows.map(({ id, version }) => ({ id: String(id), version })),
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
          SELECT id, version, kind, content, extra, source_device_id, created_at
          FROM entries
          WHERE id IN (${placeholders(ids.length)})
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
      appVersion: device.appVersion,
    };
    this.#database.prepare(`
      INSERT INTO devices (device_id, device_info, updated_at)
      VALUES (?, ?, ?)
      ON CONFLICT(device_id) DO UPDATE SET device_info = excluded.device_info, updated_at = excluded.updated_at
    `).run(device.id, JSON.stringify(deviceInfo), new Date().toISOString());
  }

  touchDevice(deviceId: string): void {
    this.#database.prepare("UPDATE devices SET updated_at = ? WHERE device_id = ?")
      .run(new Date().toISOString(), deviceId);
  }

  listDevices(): Device[] {
    return this.#listDeviceRows().map(({ device }) => device);
  }

  // WS 认证时的会话反查：拿 session 绑定的 deviceId 取设备信息，省掉客户端
  // 在 auth 消息里重复上报。
  getDevice(deviceId: string): Device | undefined {
    const row = this.#database.prepare("SELECT device_info FROM devices WHERE device_id = ?").get(deviceId) as
      | { device_info: string }
      | undefined;
    if (!row) return undefined;
    const result = DeviceSchema.safeParse({ ...JSON.parse(row.device_info), id: deviceId });
    return result.success ? result.data : undefined;
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

  // A removed device takes the entries it contributed with it, so no fresh
  // "unknown device" orphans are left behind. Returns the deleted entry ids
  // for the route layer to broadcast (pool bytes remain stored), or null when the device row
  // does not exist.
  deleteDevice(deviceId: string): string[] | null {
    const exists = this.#database.prepare("SELECT 1 FROM devices WHERE device_id = ?").get(deviceId);
    if (!exists) return null;
    return this.#transaction(() => {
      const entryIds = (this.#database.prepare(`
        DELETE FROM entries WHERE source_device_id = ?
        RETURNING id
      `).all(deviceId) as Array<{ id: number }>).map(({ id }) => String(id));
      this.#database.prepare("DELETE FROM devices WHERE device_id = ?").run(deviceId);
      return entryIds;
    });
  }

  // The server owns identity: it dedupes by content hash, assigns the rowid
  // and stamps arrival time. The client's id and clock are ignored, so a
  // retried publish cannot mint a second row. A hash conflict refreshes time;
  // increments its revision; responses and pushes use the persisted row.
  upsert(entry: EntryPublishInput): ClipboardEntry {
    const createdAt = new Date().toISOString();
    const extra = JSON.stringify({
      html: entry.html,
      rtf: entry.rtf,
      fileInfo: entry.fileInfo,
      imageInfo: entry.imageInfo,
    });
    return this.#transaction(() => {
      const row = this.#database.prepare(`
        INSERT INTO entries (
          hash, kind, content, extra, source_device_id, created_at
        ) VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(hash) DO UPDATE SET created_at = excluded.created_at, version = entries.version + 1
        RETURNING id, version, kind, content, extra, source_device_id, created_at
      `).get(
        entryHash(entry),
        entry.kind,
        entry.content,
        extra,
        entry.sourceDeviceId,
        createdAt,
      ) as EntryRow;
      const storedEntry = this.#toEntry(row)!;
      this.files.register(entryContents(storedEntry));
      return storedEntry;
    });
  }

  // Enforces the account-wide history cap: entries beyond the newest
  // `maxEntries` are dropped and their ids returned so the route layer can
  // broadcast clipboard.deleted. Like delete(), only references go away —
  // Stored bytes remain until an administrator removes them.
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
  // Stored bytes remain until an administrator removes them.
  delete(entryId: string): void {
    this.#database.prepare("DELETE FROM entries WHERE id = ?").run(entryId);
  }

  hasFileReference(entryId: string, downloadId: string): boolean {
    const row = this.#database.prepare("SELECT kind, extra FROM entries WHERE id = ?")
      .get(entryId) as { kind: string; extra: string } | undefined;
    return Boolean(row && entryContents({ kind: row.kind, ...parseExtra(row.extra) })
      .some(({ fileId }) => fileId === downloadId));
  }

  close(): void { this.#database.close(); }

  #toEntry(row: EntryRow): ClipboardEntry | undefined {
    let extra: unknown;
    try { extra = JSON.parse(row.extra); } catch { extra = {}; }
    const result = ClipboardEntrySchema.safeParse({
      ...(typeof extra === "object" && extra !== null ? extra : {}),
      id: String(row.id),
      version: row.version,
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
// prefixes keep the payload spaces disjoint. File identity includes paths,
// empty directories and repeated leaves; key insertion order is irrelevant.
function entryHash(entry: {
  kind: string;
  content: string;
  fileInfo?: FileInfo;
  imageInfo?: ImageInfo;
}): string {
  const payload = entry.kind === "text"
    ? entry.content
    : entry.kind === "files"
      ? JSON.stringify([entry.content, canonicalTree(entry.fileInfo ?? {})])
    : entryContents(entry).map(({ fileId }) => fileId).sort().join("\n") || entry.content;
  return createHash("sha256").update(`${entry.kind}\0${payload}`).digest("hex");
}

function canonicalTree(node: unknown): unknown {
  if (!node || typeof node !== "object") return node;
  return Object.fromEntries(Object.entries(node).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)
    .map(([key, value]) => [key, canonicalTree(value)]));
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
