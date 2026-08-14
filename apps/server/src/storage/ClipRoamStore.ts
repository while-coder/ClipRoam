import type { AuthResponse, ClipboardEntry, Device } from "@cliproam/protocol";
import { AccountStore } from "./AccountStore.js";
import { LegacyStorageMigrator } from "./LegacyStorageMigrator.js";
import { type StoredFile, type UploadSession, UserDataStore } from "./UserDataStore.js";

export { InvalidCredentialsError, UsernameTakenError } from "./AccountStore.js";
export type { StoredFile } from "./UserDataStore.js";

export class ClipRoamStore {
  readonly #accounts: AccountStore;
  readonly #userStores = new Map<string, UserDataStore>();

  constructor(databasePath?: string) {
    this.#accounts = new AccountStore(databasePath);
    new LegacyStorageMigrator(this.#accounts, (userId) => this.#userStore(userId)).migrate();
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

  list(userId: string): ClipboardEntry[] { return this.#userStore(userId).list(); }
  listByIds(userId: string, entryIds: readonly string[]): ClipboardEntry[] {
    const wanted = new Set(entryIds);
    return this.#userStore(userId).list().filter((entry) => wanted.has(entry.id));
  }
  upsertDevice(userId: string, device: Device): void { this.#userStore(userId).upsertDevice(device); }
  listDevices(userId: string): Device[] { return this.#userStore(userId).listDevices(); }
  upsert(userId: string, entry: ClipboardEntry): ClipboardEntry {
    return this.#userStore(userId).upsertClientEntry(entry);
  }
  entryIdForClientId(userId: string, clientId: string): string | undefined {
    return this.#userStore(userId).entryIdForClientId(clientId);
  }
  delete(userId: string, entryId: string): void { this.#userStore(userId).delete(entryId); }
  filePath(userId: string, fileId: string): string { return this.#userStore(userId).filePath(fileId); }
  storeFile(userId: string, entryId: string, fileId: string, file: StoredFile): void {
    this.#userStore(userId).storeFile(entryId, fileId, file);
  }
  getFile(userId: string, fileId: string): StoredFile | undefined { return this.#userStore(userId).getFile(fileId); }
  getUploadSession(userId: string, deviceId: string, fileFullPath: string): UploadSession | undefined {
    return this.#userStore(userId).getUploadSession(deviceId, fileFullPath);
  }
  saveUploadSession(
    userId: string,
    deviceId: string,
    fileFullPath: string,
    entryId: string,
    session: UploadSession,
  ): void { this.#userStore(userId).saveUploadSession(deviceId, fileFullPath, entryId, session); }
  deleteUploadSession(userId: string, deviceId: string, fileFullPath: string): string | undefined {
    return this.#userStore(userId).deleteUploadSession(deviceId, fileFullPath);
  }
  deleteUploadSessionsForEntry(userId: string, entryId: string): string[] {
    return this.#userStore(userId).deleteUploadSessionsForEntry(entryId);
  }

  close(): void {
    this.#accounts.close();
    this.#userStores.forEach((store) => store.close());
    this.#userStores.clear();
  }

  #userStore(userId: string): UserDataStore {
    let store = this.#userStores.get(userId);
    if (!store) {
      store = new UserDataStore(userId);
      this.#userStores.set(userId, store);
    }
    return store;
  }
}
