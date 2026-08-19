import { mkdirSync } from "node:fs";
import { dirname } from "node:path";
import Database from "better-sqlite3";

// SQLite allows 999 bound parameters by default, and one entry can reference far
// more contents than that.
export const QUERY_BATCH = 500;

export function openDatabase(path: string): Database.Database {
  mkdirSync(dirname(path), { recursive: true });
  const database = new Database(path);
  // WAL requires shared-memory files and filesystem operations that are denied
  // by some container seccomp/runtime combinations. Rollback journals avoid the
  // -wal/-shm path and are sufficient for ClipRoam's single server process.
  database.pragma("journal_mode = DELETE");
  return database;
}

export function chunk<T>(items: readonly T[], size: number): T[][] {
  const batches: T[][] = [];
  for (let index = 0; index < items.length; index += size) {
    batches.push(items.slice(index, index + size));
  }
  return batches;
}

export function withTransaction(database: Database.Database, work: () => void): void {
  database.exec("BEGIN");
  try {
    work();
    database.exec("COMMIT");
  } catch (error) {
    database.exec("ROLLBACK");
    throw error;
  }
}
