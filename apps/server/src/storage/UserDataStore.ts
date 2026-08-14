import { createHash, randomUUID } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { ClipboardEntrySchema, DeviceSchema, type ClipboardEntry, type Device } from "@cliproam/protocol";
import { userDatabasePath, userFilesDirectory } from "./DataPaths.js";

export type StoredFile = { path: string; size: number; name: string };
export type UploadSession = {
  fileId: string;
  fileSize: number;
  fileModifiedAt: number;
};
export type LegacyFileRow = {
  entry_id: string;
  file_id: string;
  path: string;
  name: string;
  size: number;
  created_at: string;
};
export type LegacyEntryRow = { id: string; payload: string; created_at: string };

export class UserDataStore {
  readonly #database: DatabaseSync;
  readonly #filesDirectory: string;

  constructor(userId: string) {
    const databasePath = userDatabasePath(userId);
    this.#filesDirectory = userFilesDirectory(userId);
    mkdirSync(dirname(databasePath), { recursive: true });
    mkdirSync(this.#filesDirectory, { recursive: true });
    this.#database = new DatabaseSync(databasePath);
    this.#database.exec(`
      PRAGMA journal_mode = WAL;

      CREATE TABLE IF NOT EXISTS clipboard_entries (
        id TEXT PRIMARY KEY,
        payload TEXT NOT NULL,
        created_at TEXT NOT NULL
      );
      CREATE INDEX IF NOT EXISTS clipboard_entries_created_at
        ON clipboard_entries(created_at DESC);

      CREATE TABLE IF NOT EXISTS client_entries (
        client_id TEXT PRIMARY KEY,
        entry_id TEXT NOT NULL UNIQUE
      );

      CREATE TABLE IF NOT EXISTS devices (
        device_id TEXT PRIMARY KEY,
        device_info TEXT NOT NULL,
        updated_at TEXT NOT NULL
      );

      CREATE TABLE IF NOT EXISTS files (
        file_id TEXT PRIMARY KEY,
        entry_id TEXT NOT NULL,
        file_key TEXT NOT NULL,
        name TEXT NOT NULL,
        size INTEGER NOT NULL,
        created_at TEXT NOT NULL
      );
      CREATE INDEX IF NOT EXISTS files_entry_id ON files(entry_id);

      CREATE TABLE IF NOT EXISTS upload_sessions (
        source_device_id TEXT NOT NULL,
        source_file_path TEXT NOT NULL,
        file_id TEXT NOT NULL UNIQUE,
        entry_id TEXT NOT NULL,
        file_size INTEGER NOT NULL,
        file_modified_at INTEGER NOT NULL,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (source_device_id, source_file_path)
      );
      CREATE INDEX IF NOT EXISTS upload_sessions_entry_id ON upload_sessions(entry_id);
    `);
    const deviceColumns = this.#database.prepare("PRAGMA table_info(devices)")
      .all() as Array<{ name: string }>;
    if (deviceColumns.some((column) => column.name === "payload")
      && !deviceColumns.some((column) => column.name === "device_info")) {
      this.#database.exec("ALTER TABLE devices RENAME COLUMN payload TO device_info");
    }
  }

  list(): ClipboardEntry[] {
    const rows = this.#database
      .prepare("SELECT payload FROM clipboard_entries ORDER BY created_at DESC")
      .all() as Array<{ payload: string }>;
    return rows.flatMap(({ payload }) => {
      const result = ClipboardEntrySchema.safeParse(JSON.parse(payload));
      return result.success ? [result.data] : [];
    });
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

  upsert(entry: ClipboardEntry): void {
    this.#database.prepare(`
      INSERT INTO clipboard_entries (id, payload, created_at)
      VALUES (?, ?, ?)
      ON CONFLICT(id) DO UPDATE SET payload = excluded.payload, created_at = excluded.created_at
    `).run(entry.id, JSON.stringify(entry), entry.createdAt);
    this.#removeUnreferencedFiles(entry.id, new Set(
      entry.files
        .filter((file) => file.location === "server")
        .map((file) => file.id),
    ));
  }

  upsertClientEntry(entry: ClipboardEntry): ClipboardEntry {
    const clientId = entry.clientId ?? entry.id;
    const existing = this.#database
      .prepare("SELECT entry_id FROM client_entries WHERE client_id = ?")
      .get(clientId) as { entry_id: string } | undefined;
    const storedEntry: ClipboardEntry = {
      ...entry,
      id: existing?.entry_id ?? randomUUID(),
      clientId,
    };
    this.upsert(storedEntry);
    this.#database.prepare(`
      INSERT INTO client_entries (client_id, entry_id)
      VALUES (?, ?)
      ON CONFLICT(client_id) DO UPDATE SET entry_id = excluded.entry_id
    `).run(clientId, storedEntry.id);
    return storedEntry;
  }

  entryIdForClientId(clientId: string): string | undefined {
    return (this.#database
      .prepare("SELECT entry_id FROM client_entries WHERE client_id = ?")
      .get(clientId) as { entry_id: string } | undefined)?.entry_id;
  }

  delete(entryId: string): void {
    this.#removeUnreferencedFiles(entryId, new Set());
    this.#database.prepare("DELETE FROM clipboard_entries WHERE id = ?").run(entryId);
    this.#database.prepare("DELETE FROM client_entries WHERE entry_id = ?").run(entryId);
  }

  filePath(fileId: string): string {
    return join(this.#filesDirectory, fileKey(fileId));
  }

  storeFile(entryId: string, fileId: string, file: StoredFile): void {
    const key = fileKey(fileId);
    this.#database.prepare(`
      INSERT INTO files (file_id, entry_id, file_key, name, size, created_at)
      VALUES (?, ?, ?, ?, ?, ?)
      ON CONFLICT(file_id) DO UPDATE SET
        entry_id = excluded.entry_id,
        file_key = excluded.file_key,
        name = excluded.name,
        size = excluded.size,
        created_at = excluded.created_at
    `).run(fileId, entryId, key, file.name, file.size, new Date().toISOString());
  }

  getFile(fileId: string): StoredFile | undefined {
    const file = this.#database
      .prepare("SELECT file_key, size, name FROM files WHERE file_id = ?")
      .get(fileId) as { file_key: string; size: number; name: string } | undefined;
    return file && { path: join(this.#filesDirectory, file.file_key), size: file.size, name: file.name };
  }

  getUploadSession(deviceId: string, fileFullPath: string): UploadSession | undefined {
    const session = this.#database.prepare(`
      SELECT file_id, file_size, file_modified_at
      FROM upload_sessions
      WHERE source_device_id = ? AND source_file_path = ?
    `).get(deviceId, fileFullPath) as {
      file_id: string;
      file_size: number;
      file_modified_at: number;
    } | undefined;
    return session && {
      fileId: session.file_id,
      fileSize: session.file_size,
      fileModifiedAt: session.file_modified_at,
    };
  }

  saveUploadSession(
    deviceId: string,
    fileFullPath: string,
    entryId: string,
    session: UploadSession,
  ): void {
    this.#database.prepare(`
      INSERT INTO upload_sessions (
        source_device_id, source_file_path, file_id, entry_id, file_size, file_modified_at, updated_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?)
      ON CONFLICT(source_device_id, source_file_path) DO UPDATE SET
        file_id = excluded.file_id,
        entry_id = excluded.entry_id,
        file_size = excluded.file_size,
        file_modified_at = excluded.file_modified_at,
        updated_at = excluded.updated_at
    `).run(
      deviceId,
      fileFullPath,
      session.fileId,
      entryId,
      session.fileSize,
      session.fileModifiedAt,
      new Date().toISOString(),
    );
  }

  deleteUploadSession(deviceId: string, fileFullPath: string): string | undefined {
    const session = this.getUploadSession(deviceId, fileFullPath);
    this.#database.prepare(`
      DELETE FROM upload_sessions WHERE source_device_id = ? AND source_file_path = ?
    `).run(deviceId, fileFullPath);
    return session?.fileId;
  }

  deleteUploadSessionsForEntry(entryId: string): string[] {
    const sessions = this.#database.prepare(`
      SELECT file_id FROM upload_sessions WHERE entry_id = ?
    `).all(entryId) as Array<{ file_id: string }>;
    this.#database.prepare("DELETE FROM upload_sessions WHERE entry_id = ?").run(entryId);
    return sessions.map((session) => session.file_id);
  }

  #removeUnreferencedFiles(entryId: string, referencedFileIds: Set<string>): void {
    const files = this.#database
      .prepare("SELECT file_id, file_key FROM files WHERE entry_id = ?")
      .all(entryId) as Array<{ file_id: string; file_key: string }>;
    const remove = this.#database.prepare("DELETE FROM files WHERE file_id = ?");
    for (const file of files) {
      if (referencedFileIds.has(file.file_id)) continue;
      rmSync(join(this.#filesDirectory, file.file_key), { force: true });
      remove.run(file.file_id);
    }
  }

  importLegacy(entries: LegacyEntryRow[], files: LegacyFileRow[]): void {
    for (const entry of entries) {
      this.#database.prepare(`
        INSERT INTO clipboard_entries (id, payload, created_at)
        VALUES (?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET payload = excluded.payload, created_at = excluded.created_at
      `).run(entry.id, entry.payload, entry.created_at);
    }
    for (const file of files) {
      const destination = this.filePath(file.file_id);
      if (existsSync(file.path) && !existsSync(destination)) copyFileSync(file.path, destination);
      this.#database.prepare(`
        INSERT INTO files (file_id, entry_id, file_key, name, size, created_at)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(file_id) DO UPDATE SET
          entry_id = excluded.entry_id,
          file_key = excluded.file_key,
          name = excluded.name,
          size = excluded.size,
          created_at = excluded.created_at
      `).run(file.file_id, file.entry_id, fileKey(file.file_id), file.name, file.size, file.created_at);
    }
  }

  close(): void { this.#database.close(); }
}

function fileKey(fileId: string): string {
  return createHash("sha256").update(fileId).digest("hex");
}
