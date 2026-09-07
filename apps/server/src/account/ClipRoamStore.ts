import type {
  AuthResponse,
  ClipboardEntry,
  Device,
  EntryManifestQuery,
  EntryManifestResponse,
  EntryPublishInput,
} from "@cliproam/protocol";
import { getLogger } from "../app/Logger.js";
import { FileStore } from "../files/FileStore.js";
import { AccountStore } from "./AccountStore.js";
import { UserDataStore } from "../clipboard/UserDataStore.js";

export { InvalidCredentialsError, UsernameTakenError } from "./AccountStore.js";

// Each UserDataStore holds an open SQLite connection, so stores left behind by
// users who have gone quiet are swept after this much inactivity. Reopening on
// the next request only costs an openDatabase plus a schema check.
const USER_STORE_IDLE_MS = 10 * 60 * 1_000;
const USER_STORE_SWEEP_INTERVAL_MS = 60 * 1_000;

const logger = getLogger("ClipRoamStore");

type TrackedUserStore = { store: UserDataStore; lastUsedAt: number };

export class ClipRoamStore {
  readonly #accounts: AccountStore;
  readonly #files: FileStore;
  readonly #userStores = new Map<string, TrackedUserStore>();
  readonly #sweepTimer: NodeJS.Timeout;

  constructor() {
    this.#accounts = new AccountStore();
    this.#files = new FileStore();
    this.#sweepTimer = setInterval(() => this.#sweepIdleStores(), USER_STORE_SWEEP_INTERVAL_MS);
    this.#sweepTimer.unref();
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
  upsert(userId: string, entry: EntryPublishInput): ClipboardEntry {
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
    return this.#files.reclaimUnreferenced(referenced, partialTtlMs);
  }
  close(): void {
    clearInterval(this.#sweepTimer);
    this.#accounts.close();
    this.#files.close();
    this.#userStores.forEach((tracked) => tracked.store.close());
    this.#userStores.clear();
  }

  // better-sqlite3 is synchronous, so closing a store mid-request is not a
  // race: the sweeper only runs between requests on the event loop.
  #sweepIdleStores(): void {
    const now = Date.now();
    let swept = 0;
    for (const [userId, tracked] of this.#userStores) {
      if (now - tracked.lastUsedAt < USER_STORE_IDLE_MS) continue;
      tracked.store.close();
      this.#userStores.delete(userId);
      swept += 1;
    }
    if (swept > 0) logger.info(`Closed ${swept} idle user stores`);
  }

  #userStore(userId: string): UserDataStore {
    let tracked = this.#userStores.get(userId);
    if (!tracked) {
      tracked = { store: new UserDataStore(userId, this.#files), lastUsedAt: 0 };
      this.#userStores.set(userId, tracked);
    }
    tracked.lastUsedAt = Date.now();
    return tracked.store;
  }

}
