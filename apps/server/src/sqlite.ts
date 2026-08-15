import type { DatabaseSync } from "node:sqlite";

// SQLite allows 999 bound parameters by default, and one entry can reference far
// more contents than that.
export const QUERY_BATCH = 500;

export function chunk<T>(items: readonly T[], size: number): T[][] {
  const batches: T[][] = [];
  for (let index = 0; index < items.length; index += size) {
    batches.push(items.slice(index, index + size));
  }
  return batches;
}

export function withTransaction(database: DatabaseSync, work: () => void): void {
  database.exec("BEGIN");
  try {
    work();
    database.exec("COMMIT");
  } catch (error) {
    database.exec("ROLLBACK");
    throw error;
  }
}
