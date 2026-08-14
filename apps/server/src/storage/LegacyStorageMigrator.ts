import { existsSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { legacyDatabasePaths } from "./DataPaths.js";
import { AccountStore, type LegacySessionRow, type LegacyUserRow } from "./AccountStore.js";
import { type LegacyEntryRow, type LegacyFileRow, type UserDataStore } from "./UserDataStore.js";

const migrationId = "legacy-single-database-v1";
const migrationStartedId = `${migrationId}:started`;

export class LegacyStorageMigrator {
  constructor(
    private readonly accounts: AccountStore,
    private readonly getUserStore: (userId: string) => UserDataStore,
  ) {}

  migrate(): void {
    if (this.accounts.hasMigration(migrationId)) return;
    const migrationStarted = this.accounts.hasMigration(migrationStartedId);
    if (!migrationStarted && this.accounts.hasUsers()) return;

    const legacyPath = legacyDatabasePaths().find(existsSync);
    if (!legacyPath) return;

    const legacy = new DatabaseSync(legacyPath);
    try {
      if (!this.#hasRequiredTables(legacy)) return;
      if (!migrationStarted) this.accounts.markMigration(migrationStartedId);

      const users = legacy.prepare(
        "SELECT id, username, password_hash, password_salt, created_at FROM users",
      ).all() as LegacyUserRow[];
      for (const user of users) {
        this.accounts.importLegacyUser(user);
        this.getUserStore(user.id).importLegacy(
          legacy.prepare("SELECT id, payload, created_at FROM user_clipboard_entries WHERE user_id = ?")
            .all(user.id) as LegacyEntryRow[],
          legacy.prepare("SELECT entry_id, file_id, path, name, size, created_at FROM user_files WHERE user_id = ?")
            .all(user.id) as LegacyFileRow[],
        );
      }

      this.accounts.importLegacySessions(
        legacy.prepare("SELECT token_hash, user_id, expires_at, created_at FROM sessions").all() as LegacySessionRow[],
      );
      this.accounts.markMigration(migrationId);
      this.accounts.removeMigration(migrationStartedId);
    } finally {
      legacy.close();
    }
  }

  #hasRequiredTables(database: DatabaseSync): boolean {
    return ["users", "user_clipboard_entries", "user_files"].every((table) => Boolean(
      database.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?").get(table),
    ));
  }
}
