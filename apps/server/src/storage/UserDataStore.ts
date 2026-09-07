import {
  ClipboardEntrySchema,
  ClipboardTreeSchema,
  DeviceSchema,
  type ClipboardEntry,
  type ClipboardFile,
  type ClipboardManifestEntry,
  type ClipboardTree,
  type Device,
  type EntryManifestQuery,
} from "@cliproam/protocol";
import type Database from "better-sqlite3";
import { FileStore } from "../files/FileStore.js";
import { chunk, openDatabase, QUERY_BATCH, withTransaction } from "../sqlite.js";
import { userDatabasePath } from "./DataPaths.js";

// The server-wide content pool changed layout. Older per-user entries are not
// retained because their content metadata belongs to the discarded pool.
const SCHEMA_VERSION = 4;

type EntryRow = {
  id: string;
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
    const version = (this.#database.prepare("PRAGMA user_version").get() as
      { user_version: number } | undefined)?.user_version ?? 0;
    if (version !== SCHEMA_VERSION) {
      this.#database.exec(`
        DROP TABLE IF EXISTS clipboard_entries;
        DROP TABLE IF EXISTS files;
        DROP TABLE IF EXISTS upload_sessions;
      `);
    }
    this.#database.exec(`
      CREATE TABLE IF NOT EXISTS clipboard_entries (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        content TEXT NOT NULL,
        extra TEXT NOT NULL DEFAULT '{}',
        source_device_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        pinned INTEGER NOT NULL
      );
      -- The list endpoint orders on the (created_at, id) pair; id being the
      -- primary key makes the order total. SQLite satisfies
      -- ORDER BY ... DESC by scanning this backwards.
      DROP INDEX IF EXISTS clipboard_entries_created_at;
      CREATE INDEX IF NOT EXISTS clipboard_entries_order
        ON clipboard_entries(created_at, id);

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

  // Offset pagination over entry identities with optional keyword and UTC
  // date-range filters. One extra row is fetched to decide hasMore without a
  // separate count query. Full details ride POST /entries/query.
  listManifestPage(query: EntryManifestQuery, limit: number): { manifest: ClipboardManifestEntry[]; hasMore: boolean } {
    const rows = this.#database
      .prepare(`
        SELECT id
        FROM clipboard_entries
        WHERE (@search IS NULL OR content LIKE @search ESCAPE '\\')
          AND (@dayStart IS NULL OR created_at BETWEEN @dayStart AND @dayEnd)
        ORDER BY created_at DESC, id DESC
        LIMIT @limit OFFSET @offset
      `)
      .all({
        search: query.search ? `%${escapeLike(query.search)}%` : null,
        dayStart: query.dateStart ? `${query.dateStart}T00:00:00.000Z` : null,
        dayEnd: query.dateEnd ? `${query.dateEnd}T23:59:59.999Z` : null,
        limit: limit + 1,
        offset: ((query.page ?? 1) - 1) * limit,
      }) as Array<{ id: string }>;
    const hasMore = rows.length > limit;
    return { manifest: rows.slice(0, limit), hasMore };
  }

  listByIds(entryIds: readonly string[]): ClipboardEntry[] {
    const entries: ClipboardEntry[] = [];
    for (const batch of chunk([...new Set(entryIds)], QUERY_BATCH)) {
      const rows = this.#database
        .prepare(`
          SELECT id, kind, content, extra, source_device_id, created_at, pinned
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
    const storedEntry = entry;
    this.#transaction(() => {
      this.#database.prepare(`
        INSERT INTO clipboard_entries (
          id, kind, content, extra, source_device_id, created_at, pinned
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
          kind = excluded.kind,
          content = excluded.content,
          extra = excluded.extra,
          source_device_id = excluded.source_device_id,
          created_at = excluded.created_at,
          pinned = excluded.pinned
      `).run(
        storedEntry.id,
        storedEntry.kind,
        storedEntry.content,
        JSON.stringify({
          html: storedEntry.html,
          rtf: storedEntry.rtf,
          thumbnail: storedEntry.thumbnail,
          tree: storedEntry.tree,
        }),
        storedEntry.sourceDeviceId,
        storedEntry.createdAt,
        Number(storedEntry.pinned),
      );
      this.files.register(storedEntry.files);
    });
    return { ...storedEntry, files: this.#contentsOf(storedEntry.tree) };
  }

  // Content is shared across entries, so deletion only drops the reference.
  // Unreferenced bytes are reclaimed by collectGarbage().
  delete(entryId: string): void {
    this.#database.prepare("DELETE FROM clipboard_entries WHERE id = ?").run(entryId);
  }

  // The server-wide pool owns collection. This returns this account's mark set
  // so ClipRoamStore can union it with every other account before sweeping.
  referencedFileIds(): Set<string> {
    const referenced = new Set<string>();
    const rows = this.#database.prepare("SELECT extra FROM clipboard_entries").all() as Array<{ extra: string }>;
    for (const row of rows) {
      for (const node of parseExtra(row.extra).tree?.files ?? []) referenced.add(node.f);
    }
    return referenced;
  }

  hasFileReference(entryId: string, fileId: string): boolean {
    const row = this.#database.prepare("SELECT extra FROM clipboard_entries WHERE id = ?")
      .get(entryId) as { extra: string } | undefined;
    return Boolean(row && parseExtra(row.extra).tree?.files.some((node) => node.f === fileId));
  }

  close(): void { this.#database.close(); }

  #toEntry(row: EntryRow): ClipboardEntry | undefined {
    const extra = parseExtra(row.extra);
    const result = ClipboardEntrySchema.safeParse({
      html: extra.html,
      rtf: extra.rtf,
      thumbnail: extra.thumbnail,
      tree: extra.tree,
      id: row.id,
      kind: row.kind,
      content: row.content,
      files: this.#contentsOf(extra.tree),
      sourceDeviceId: row.source_device_id,
      createdAt: row.created_at,
      pinned: Boolean(row.pinned),
    });
    return result.success ? result.data : undefined;
  }

  // The tree is the only record of which contents an entry uses; the pool
  // supplies size and availability for each distinct reference.
  #contentsOf(tree: ClipboardTree | undefined): ClipboardFile[] {
    return this.files.describe([...new Set((tree?.files ?? []).map((node) => node.f))]);
  }

  #transaction(work: () => void): void {
    withTransaction(this.#database, work);
  }
}

// LIKE wildcards in user input must match literally, so they are escaped and
// the statement declares '\\' as the escape character.
function escapeLike(value: string): string {
  return value.replace(/[\\%_]/g, (character) => `\\${character}`);
}

function parseExtra(extra: string): { html?: string; rtf?: string; thumbnail?: string; tree?: ClipboardTree } {
  let raw: { html?: unknown; rtf?: unknown; thumbnail?: unknown; tree?: unknown };
  try {
    raw = JSON.parse(extra) as typeof raw;
  } catch {
    return {};
  }
  const tree = raw.tree === undefined ? undefined : ClipboardTreeSchema.safeParse(raw.tree);
  return {
    html: typeof raw.html === "string" ? raw.html : undefined,
    rtf: typeof raw.rtf === "string" ? raw.rtf : undefined,
    thumbnail: typeof raw.thumbnail === "string" && raw.thumbnail.length <= 96 * 1024
      ? raw.thumbnail
      : undefined,
    tree: tree?.success ? tree.data : undefined,
  };
}
