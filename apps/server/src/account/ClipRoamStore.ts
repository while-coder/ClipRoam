import { rm } from "node:fs/promises";
import type {
  AuthResponse,
  ClipboardEntry,
  Device,
  EntryManifestQuery,
  EntryManifestResponse,
  EntryPublishInput,
} from "@cliproam/protocol";
import { getLogger } from "../app/Logger.js";
import { USER_STORE_IDLE_MS, USER_STORE_SWEEP_INTERVAL_MS } from "../app/ServerConfig.js";
import { FileStore } from "../files/FileStore.js";
import { userDirectory } from "../DataPaths.js";
import { AccountStore, type AdminUserSummary } from "./AccountStore.js";
import { UserDataStore } from "../clipboard/UserDataStore.js";

// `AuthResponse` 去掉服务器附带字段：`settings` 由路由层注入。
type AuthSession = Omit<AuthResponse, "settings">;

export { InvalidCredentialsError, UsernameTakenError } from "./AccountStore.js";

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

  async register(username: string, password: string, deviceId: string): Promise<AuthSession> {
    const response = await this.#accounts.register(username, password, deviceId);
    this.#userStore(response.user.id);
    return response;
  }

  login(username: string, password: string, deviceId: string): Promise<AuthSession> {
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
  pruneHistory(userId: string, maxEntries: number): string[] { return this.#userStore(userId).prune(maxEntries); }
  files(): FileStore { return this.#files; }
  listUsers(search?: string): AdminUserSummary[] { return this.#accounts.listUsers(search); }
  hasUser(userId: string): boolean { return this.#accounts.hasUser(userId); }
  resetPassword(userId: string, newPassword: string): Promise<boolean> {
    return this.#accounts.resetPassword(userId, newPassword);
  }

  listUserDevices(userId: string): Array<Device & { lastSeenAt: string }> {
    return this.#userStore(userId).listDevicesWithLastSeen();
  }

  // The accounts-side session for this device dies with the device row, so a
  // removed device cannot keep syncing until its stored token expires.
  deleteUserDevice(userId: string, deviceId: string): boolean {
    this.#accounts.deleteSession(userId, deviceId);
    return this.#userStore(userId).deleteDevice(deviceId);
  }

  // The account row cascades its sessions; the per-user database and directory
  // are removed with it, while pool files it referenced are reclaimed by the
  // next garbage-collection sweep.
  deleteUser(userId: string): boolean {
    this.#userStores.get(userId)?.store.close();
    this.#userStores.delete(userId);
    const removed = this.#accounts.deleteUser(userId);
    if (removed) {
      void rm(userDirectory(userId), { recursive: true, force: true })
        .catch((error) => logger.error(`Failed to remove data directory for user ${userId}:`, error));
    }
    return removed;
  }
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
