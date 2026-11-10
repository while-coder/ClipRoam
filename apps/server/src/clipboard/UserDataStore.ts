import { createHash, randomUUID } from "node:crypto";
import {
  ClipboardEntrySchema,
  FileInfoSchema,
  ImageInfoSchema,
  entryContents,
  type ClipboardEntry,
  type ClipboardManifestEntry,
  type Device,
  type EntryManifestQuery,
  type EntryPublishInput,
  type EntryCollection,
  type FileInfo,
  type ImageInfo,
  type FavoriteFolder,
  type FavoriteFolderInput,
} from "@cliproam/protocol";
import type Database from "better-sqlite3";
import { FileStore } from "../files/FileStore.js";
import { chunk, escapeLike, openDatabase, placeholders, QUERY_BATCH, withTransaction } from "../sqlite.js";
import { userDatabasePath } from "../DataPaths.js";

type EntryRow = {
  id: number | string;
  version: number;
  favorite_id?: string | null;
  sticky_id?: string | null;
  favorite_folder_id: string | null;
  kind: string;
  content: string;
  extra: string;
  source_device_id: string;
  created_at: string;
};

const collectionTables = { history: "entries", favorites: "favorites", sticky: "sticky_notes" } as const;

// Clipboard records and devices. Contents live in an independent pool that this
// store only references by id, so a tree is the sole record of which bytes an
// entry needs.
export class UserDataStore {
  readonly #database: Database.Database;

  constructor(userId: string, readonly files: FileStore, databasePath = userDatabasePath(userId)) {
    this.#database = openDatabase(databasePath);
    this.#applySchema();
  }

  #applySchema(): void {
    this.#database.exec(`
      CREATE TABLE IF NOT EXISTS entries (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
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
        last_online_at TEXT
      );

      CREATE TABLE IF NOT EXISTS favorite_folders (
        id TEXT PRIMARY KEY,
        parent_id TEXT,
        name TEXT NOT NULL
      );
      CREATE UNIQUE INDEX IF NOT EXISTS favorite_folders_name
        ON favorite_folders (COALESCE(parent_id, ''), name);
    `);
    for (const table of ["favorites", "sticky_notes"] as const) {
      this.#database.exec(`CREATE TABLE IF NOT EXISTS ${table} (
        id TEXT PRIMARY KEY,
        version INTEGER NOT NULL DEFAULT 1,
        hash TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL,
        content TEXT NOT NULL,
        extra TEXT NOT NULL DEFAULT '{}',
        source_device_id TEXT NOT NULL,
        created_at TEXT NOT NULL
        ${table === "favorites" ? ", favorite_folder_id TEXT" : ""}
      ); CREATE INDEX IF NOT EXISTS ${table}_created_at ON ${table} (created_at DESC)`);
    }
    this.#database.exec("CREATE INDEX IF NOT EXISTS favorites_folder ON favorites (favorite_folder_id)");
    // Remove legacy cross-collection writes. Membership is derived when reading;
    // saving, moving or deleting a copy never changes another record's revision.
    for (const table of ["favorites", "sticky_notes"] as const) {
      for (const event of ["INSERT", "UPDATE", "DELETE"] as const) {
        this.#database.exec(`DROP TRIGGER IF EXISTS ${table}_history_${event.toLowerCase()}`);
      }
    }
  }

  // Offset pagination over entry identities with optional keyword, UTC
  // date-range, kind and source-device filters. The total covers the same
  // filters, so a client can render "page x of y" and detect the last page
  // from a filtered result set. Full details ride POST /entries/query.
  // Recency order follows `created_at`: the rowid no longer reflects it, since
  // a re-copy refreshes the timestamp while keeping its original rowid.
  listHistoryManifestPage(query: EntryManifestQuery, limit: number) {
    // A folder filter never switches a history query to another collection.
    return this.#listManifestPage("entries", { ...query, favoriteFolderId: undefined }, limit);
  }

  listFavoriteManifestPage(query: EntryManifestQuery, limit: number) {
    return this.#listManifestPage("favorites", query, limit);
  }

  #listManifestPage(table: "entries" | "favorites", query: EntryManifestQuery, limit: number): { manifest: ClipboardManifestEntry[]; total: number } {
    // The device filter matches entries whose source is one of the selected
    // devices. An absent filter drops the clause entirely.
    const selectedDeviceIds = [...new Set(query.deviceIds ?? [])];
    const deviceParams = Object.fromEntries(selectedDeviceIds.map((id, index) => [`device${index}`, id]));
    const deviceClause = selectedDeviceIds.length
      ? `e.source_device_id IN (${Object.keys(deviceParams).map((name) => `@${name}`).join(", ")})`
      : "";
    const folderIds = query.favoriteFolderId && query.favoriteFolderId !== "unfiled"
      ? this.#folderSubtree(query.favoriteFolderId) : [];
    const folderParams = Object.fromEntries(folderIds.map((id, index) => [`folder${index}`, id]));
    const folderClause = query.favoriteFolderId === "unfiled"
      ? "e.favorite_folder_id IS NULL"
      : folderIds.length ? `e.favorite_folder_id IN (${Object.keys(folderParams).map((name) => `@${name}`).join(", ")})` : "";
    // Shared between the page read and the total count so the two can never
    // disagree about what a "matching row" is.
    const where = `
      WHERE (@search IS NULL OR e.content LIKE @search ESCAPE '\\')
        AND (@dateStart IS NULL OR e.created_at BETWEEN @dateStart AND @dateEnd)
        AND (@kind IS NULL OR e.kind = @kind)
        ${folderClause ? `AND (${folderClause})` : ""}
        ${deviceClause ? `AND (${deviceClause})` : ""}
    `;
    const filters = {
      search: query.search ? `%${escapeLike(query.search)}%` : null,
      dateStart: query.dateStart ? normalizeDateBound(query.dateStart, "start") : null,
      dateEnd: query.dateEnd ? normalizeDateBound(query.dateEnd, "end") : null,
      kind: query.kind ?? null,
      ...deviceParams,
      ...folderParams,
    };
    const rows = this.#database
      .prepare(`
        SELECT e.id, e.version,
          ${table === "favorites" ? "e.id" : "f.id"} AS favoriteId,
          s.id AS stickyId
        FROM ${table} e
        LEFT JOIN sticky_notes s ON s.hash = e.hash
        ${table === "entries" ? "LEFT JOIN favorites f ON f.hash = e.hash" : ""}
        ${where}
        ORDER BY e.created_at DESC, e.id DESC
        LIMIT @limit OFFSET @offset
      `)
      .all({ ...filters, limit, offset: ((query.page ?? 1) - 1) * limit }) as Array<{
        id: number | string; version: number; favoriteId: string | null; stickyId: string | null;
      }>;
    const { count } = this.#database
      .prepare(`SELECT COUNT(*) AS count FROM ${table} e ${where}`)
      .get(filters) as { count: number };
    return {
      manifest: rows.map((row) => ({ ...row, id: String(row.id), collection: table === "entries" ? "history" : "favorites" })),
      total: count,
    };
  }

  listByIds(collection: EntryCollection, entryIds: readonly string[]): ClipboardEntry[] {
    const table = collectionTables[collection];
    const entries: ClipboardEntry[] = [];
    for (const batch of chunk([...new Set(entryIds)], QUERY_BATCH)) {
      const rows = this.#database.prepare(`
          SELECT e.*,
            ${collection === "favorites" ? "e.id" : "f.id"} AS favorite_id,
            ${collection === "sticky" ? "e.id" : "s.id"} AS sticky_id,
            ${table === "favorites" ? "e.favorite_folder_id" : "NULL"} AS favorite_folder_id
          FROM ${table} e
          ${collection === "sticky" ? "" : "LEFT JOIN sticky_notes s ON s.hash = e.hash"}
          ${collection === "favorites" ? "" : "LEFT JOIN favorites f ON f.hash = e.hash"}
          WHERE e.id IN (${placeholders(batch.length)})
          ORDER BY e.created_at DESC
        `).all(...batch) as EntryRow[];
      for (const row of rows) {
        const entry = this.#toEntry(row, collection);
        if (entry) entries.push(entry);
      }
    }
    return entries;
  }

  upsertDevice(device: Device): void {
    this.#database.prepare(`
      INSERT INTO devices (device_id, device_info, last_online_at)
      VALUES (?, ?, ?)
      ON CONFLICT(device_id) DO UPDATE SET
        device_info = excluded.device_info,
        last_online_at = COALESCE(devices.last_online_at, excluded.last_online_at)
    `).run(device.id, device.deviceInfo, new Date().toISOString());
  }

  recordDeviceOnline(deviceId: string): string {
    const now = new Date().toISOString();
    this.#database.prepare("UPDATE devices SET last_online_at = ? WHERE device_id = ?")
      .run(now, deviceId);
    return now;
  }

  listDevices(): Device[] {
    return this.#listDeviceRows();
  }

  // WS 认证时的会话反查：拿 session 绑定的 deviceId 取设备信息，省掉客户端
  // 在 auth 消息里重复上报。
  getDevice(deviceId: string): Device | undefined {
    const row = this.#database.prepare("SELECT device_info, last_online_at FROM devices WHERE device_id = ?").get(deviceId) as
      | { device_info: string; last_online_at: string | null }
      | undefined;
    if (!row) return undefined;
    return { id: deviceId, deviceInfo: row.device_info, lastOnlineAt: row.last_online_at };
  }

  #listDeviceRows(): Device[] {
    const rows = this.#database.prepare("SELECT device_id, device_info, last_online_at FROM devices ORDER BY last_online_at DESC, device_id")
      .all() as Array<{ device_id: string; device_info: string; last_online_at: string | null }>;
    return rows.map(({ device_id, device_info, last_online_at }) => ({
      id: device_id,
      deviceInfo: device_info,
      lastOnlineAt: last_online_at,
    }));
  }

  // Removing a device leaves its account-owned clipboard history intact.
  removeDevice(deviceId: string): boolean {
    return this.#database.prepare("DELETE FROM devices WHERE device_id = ?").run(deviceId).changes > 0;
  }

  // Deleting a device's history preserves its independent favorite/sticky copies.
  // Returns history ids for deletion pushes, or null if the device does not exist.
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
        RETURNING id
      `).get(
        entryHash(entry),
        entry.kind,
        entry.content,
        extra,
        entry.sourceDeviceId,
        createdAt,
      ) as { id: number };
      const storedEntry = this.listByIds("history", [String(row.id)])[0]!;
      this.files.register(entryContents(storedEntry));
      return storedEntry;
    });
  }

  setFavorite(collection: EntryCollection, entryIds: readonly string[], isFavorite: boolean, favoriteFolderId?: string | null): ClipboardEntry[] {
    return this.#transaction(() => {
      if (!isFavorite) return this.#removeSaved("favorites", entryIds);
      if (favoriteFolderId) this.#requireFolder(favoriteFolderId);
      return this.#copyTo("favorites", collection, entryIds, favoriteFolderId);
    });
  }

  listSticky(): ClipboardEntry[] {
    const rows = this.#database.prepare(`
      SELECT s.*, f.id AS favorite_id, s.id AS sticky_id, NULL AS favorite_folder_id
      FROM sticky_notes s
      LEFT JOIN favorites f ON f.hash = s.hash
      ORDER BY s.created_at DESC, s.id DESC
    `).all() as EntryRow[];
    return rows.flatMap((row) => { const entry = this.#toEntry(row, "sticky"); return entry ? [entry] : []; });
  }

  setSticky(collection: EntryCollection, entryIds: readonly string[], isSticky: boolean, maxEntries: number): ClipboardEntry[] {
    return this.#transaction(() => {
      if (!isSticky) return this.#removeSaved("sticky_notes", entryIds);
      return this.#copyTo("sticky_notes", collection, entryIds, undefined, maxEntries);
    });
  }

  // Each destination owns its content snapshot and identity. Hash deduplication
  // makes repeated saves idempotent without retaining a link to the source row.
  #copyTo(table: "favorites" | "sticky_notes", collection: EntryCollection, entryIds: readonly string[], folderId?: string | null, maxEntries = Infinity): ClipboardEntry[] {
    const ids = [...new Set(entryIds)];
    const sources = this.listByIds(collection, ids);
    if (sources.length !== ids.length) throw new StickyEntryError("剪贴板记录不存在", 404);
    let { count } = this.#database.prepare(`SELECT COUNT(*) AS count FROM ${table}`).get() as { count: number };
    const savedIds = new Set<string>();
    for (const source of sources) {
      const hash = entryHash(source);
      let saved = this.#database.prepare(`SELECT id FROM ${table} WHERE hash = ?`).get(hash) as { id: string } | undefined;
      if (!saved) {
        if (count >= maxEntries) throw new StickyEntryError(`便签数量已达上限（${maxEntries} 条），请先取消其他便签`, 409);
        const favorite = table === "favorites";
        saved = { id: randomUUID() };
        this.#database.prepare(`INSERT INTO ${table}
          (id, hash, kind, content, extra, source_device_id, created_at${favorite ? ", favorite_folder_id" : ""})
          VALUES (?, ?, ?, ?, ?, ?, ?${favorite ? ", ?" : ""})`).run(
          saved.id, hash, source.kind, source.content,
          JSON.stringify({ html: source.html, rtf: source.rtf, fileInfo: source.fileInfo, imageInfo: source.imageInfo }),
          source.sourceDeviceId, new Date().toISOString(), ...(favorite ? [folderId ?? null] : []),
        );
        count++;
      } else if (table === "favorites" && folderId !== undefined) {
        this.#database.prepare("UPDATE favorites SET favorite_folder_id = ?, version = version + 1 WHERE id = ?").run(folderId, saved.id);
      }
      savedIds.add(saved.id);
    }
    return this.listByIds(table === "favorites" ? "favorites" : "sticky", [...savedIds]);
  }

  // Cancellation deletes only destination records; it never alters history or
  // the other saved collection. Return deleted snapshots for push notifications.
  // Only the destination's own ids can remove its snapshots.
  #removeSaved(table: "favorites" | "sticky_notes", ids: readonly string[]): ClipboardEntry[] {
    const uniqueIds = [...new Set(ids)];
    if (!uniqueIds.length) return [];
    const savedIds = (this.#database.prepare(`
      SELECT id FROM ${table} WHERE id IN (${placeholders(uniqueIds.length)})
    `).all(...uniqueIds) as Array<{ id: string }>).map(({ id }) => id);
    const entries = this.listByIds(table === "favorites" ? "favorites" : "sticky", savedIds);
    for (const batch of chunk(entries, QUERY_BATCH)) {
      this.#database.prepare(`DELETE FROM ${table} WHERE id IN (${placeholders(batch.length)})`).run(...batch.map(({ id }) => id));
    }
    return entries;
  }

  listFavoriteFolders(): FavoriteFolder[] {
    return this.#database.prepare(`
      SELECT f.id, f.parent_id AS parentId, f.name, COUNT(e.id) AS entryCount
      FROM favorite_folders f LEFT JOIN favorites e ON e.favorite_folder_id = f.id
      GROUP BY f.id ORDER BY f.name COLLATE NOCASE, f.id
    `).all() as FavoriteFolder[];
  }

  saveFavoriteFolder(input: FavoriteFolderInput, id: string = randomUUID()): void {
    this.#transaction(() => {
      const folders = this.listFavoriteFolders();
      if (input.parentId) this.#requireFolder(input.parentId);
      const subtree = folders.some((folder) => folder.id === id) ? this.#folderSubtree(id) : [id];
      if (input.parentId && subtree.includes(input.parentId)) throw new FavoriteFolderError("不能移动到自身或下级目录");
      if (folders.some((folder) => folder.id !== id && folder.parentId === input.parentId && folder.name === input.name)) {
        throw new FavoriteFolderError("同级目录名称已存在", 409);
      }
      this.#database.prepare(`INSERT INTO favorite_folders (id, parent_id, name) VALUES (?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET parent_id = excluded.parent_id, name = excluded.name`).run(id, input.parentId, input.name);
    });
  }

  requireFavoriteFolder(id: string): void { this.#requireFolder(id); }

  deleteFavoriteFolder(id: string): void {
    this.#transaction(() => {
      const ids = this.#folderSubtree(id);
      for (const batch of chunk(ids, QUERY_BATCH)) {
        this.#database.prepare(`UPDATE favorites SET favorite_folder_id = NULL, version = version + 1
          WHERE favorite_folder_id IN (${placeholders(batch.length)})`).run(...batch);
        this.#database.prepare(`DELETE FROM favorite_folders WHERE id IN (${placeholders(batch.length)})`).run(...batch);
      }
    });
  }

  #requireFolder(id: string): void {
    if (!this.#database.prepare("SELECT id FROM favorite_folders WHERE id = ?").get(id)) {
      throw new FavoriteFolderError("目录不存在或已被删除，请重新选择", 404);
    }
  }

  #folderSubtree(id: string): string[] {
    this.#requireFolder(id);
    return (this.#database.prepare(`WITH RECURSIVE subtree(id) AS (
      SELECT id FROM favorite_folders WHERE id = ?
      UNION ALL SELECT f.id FROM favorite_folders f JOIN subtree s ON f.parent_id = s.id
    ) SELECT id FROM subtree`).all(id) as Array<{ id: string }>).map((folder) => folder.id);
  }

  // Enforces the history cap. Saved copies live in separate tables.
  // Entries beyond the newest `maxEntries` are dropped and returned so routes can
  // broadcast clipboard.deleted. Like delete(), only references go away —
  // Stored bytes remain until an administrator removes them.
  prune(maxEntries: number): string[] {
    if (maxEntries <= 0) return [];
    const rows = this.#database.prepare(`
      DELETE FROM entries
      WHERE id NOT IN (
        SELECT id FROM entries ORDER BY created_at DESC, id DESC LIMIT ?
      )
      RETURNING id
    `).all(maxEntries) as Array<{ id: number }>;
    return rows.map(({ id }) => String(id));
  }

  // Content is shared across entries, so deletion only drops the reference.
  // Stored bytes remain until an administrator removes them.
  delete(collection: EntryCollection, entryIds: readonly string[]): void {
    const table = collectionTables[collection];
    this.#transaction(() => {
      for (const batch of chunk([...new Set(entryIds)], QUERY_BATCH)) {
        this.#database.prepare(`DELETE FROM ${table} WHERE id IN (${placeholders(batch.length)})`).run(...batch);
      }
    });
  }

  hasFileReference(collection: EntryCollection, entryId: string, downloadId: string): boolean {
    const row = this.#database.prepare(`SELECT kind, extra FROM ${collectionTables[collection]} WHERE id = ?`)
      .get(entryId) as { kind: string; extra: string } | undefined;
    return Boolean(row && entryContents({ kind: row.kind, ...parseExtra(row.extra) })
      .some(({ fileId }) => fileId === downloadId));
  }

  close(): void { this.#database.close(); }

  #toEntry(row: EntryRow, collection: EntryCollection): ClipboardEntry | undefined {
    let extra: unknown;
    try { extra = JSON.parse(row.extra); } catch { extra = {}; }
    const result = ClipboardEntrySchema.safeParse({
      ...(typeof extra === "object" && extra !== null ? extra : {}),
      id: String(row.id),
      collection,
      version: row.version,
      favoriteId: row.favorite_id ?? null,
      stickyId: row.sticky_id ?? null,
      favoriteFolderId: row.favorite_folder_id ?? null,
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

export class FavoriteFolderError extends Error {
  constructor(message: string, readonly statusCode = 400) { super(message); }
}

export class StickyEntryError extends Error {
  constructor(message: string, readonly statusCode = 400) { super(message); }
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
