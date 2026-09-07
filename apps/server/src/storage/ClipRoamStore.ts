import type {
  AuthResponse,
  ClipboardEntry,
  Device,
  EntryManifestQuery,
  EntryManifestResponse,
} from "@cliproam/protocol";
import type Database from "better-sqlite3";
import { FileStore } from "../files/FileStore.js";
import { openDatabase } from "../sqlite.js";
import { AccountStore } from "./AccountStore.js";
import { filesDatabasePath, filesDirectory } from "./DataPaths.js";
import { UserDataStore } from "./UserDataStore.js";

export { InvalidCredentialsError, UsernameTakenError } from "./AccountStore.js";

export class ClipRoamStore {
  readonly #accounts: AccountStore;
  readonly #filesDatabase: Database.Database;
  readonly #files: FileStore;
  readonly #userStores = new Map<string, UserDataStore>();

  constructor(databasePath?: string) {
    this.#accounts = new AccountStore(databasePath);
    this.#filesDatabase = openDatabase(filesDatabasePath);
    this.#files = new FileStore(this.#filesDatabase, filesDirectory);
    this.#files.applySchema();
  }

  async register(username: string, password: string, deviceId: string): Promise<AuthResponse> {
    const response = await this.#accounts.register(username, password, deviceId);
    this.#userStore(response.user.id);
    return response;
  }

  login(username: string, password: string, deviceId: string): Promise<AuthResponse> {
    return this.#accounts.login(username, password, deviceId);
  }

  changePassword(userId: string, currentPassword: string, newPassword: string): Promise<void> {
    return this.#accounts.changePassword(userId, currentPassword, newPassword);
  }

  authenticateSession(token: string): { id: string; username: string } | undefined {
    return this.#accounts.authenticateSession(token);
  }

  listManifestPage(userId: string, query: EntryManifestQuery, limit: number): EntryManifestResponse {
    return this.#userStore(userId).listManifestPage(query, limit);
  }
  listByIds(userId: string, entryIds: readonly string[]): ClipboardEntry[] {
    return this.#userStore(userId).listByIds(entryIds);
  }
  upsertDevice(userId: string, device: Device): void { this.#userStore(userId).upsertDevice(device); }
  listDevices(userId: string): Device[] { return this.#userStore(userId).listDevices(); }
  upsert(userId: string, entry: ClipboardEntry): ClipboardEntry {
    return this.#userStore(userId).upsert(entry);
  }
  delete(userId: string, entryId: string): void { this.#userStore(userId).delete(entryId); }
  files(): FileStore { return this.#files; }
  canReadFile(userId: string, entryId: string, fileId: string): boolean {
    return this.#userStore(userId).hasFileReference(entryId, fileId);
  }
  collectGarbage(partialTtlMs: number): { removedFiles: number; removedBytes: number } {
    const referenced = new Set<string>();
    for (const userId of this.#accounts.listUserIds()) {
      for (const fileId of this.#userStore(userId).referencedFileIds()) referenced.add(fileId);
    }
    return this.#files.sweep(referenced, partialTtlMs);
  }
  close(): void {
    this.#accounts.close();
    this.#filesDatabase.close();
    this.#userStores.forEach((store) => store.close());
    this.#userStores.clear();
  }

  #userStore(userId: string): UserDataStore {
    let store = this.#userStores.get(userId);
    if (!store) {
      store = new UserDataStore(userId, this.#files);
      this.#userStores.set(userId, store);
    }
    return store;
  }

}
