import type {
  AuthResponse,
  ClipboardEntry,
  ClipboardManifestEntry,
  Device,
} from "@cliproam/protocol";
import { AccountStore } from "./AccountStore.js";
import { UserDataStore } from "./UserDataStore.js";

export { InvalidCredentialsError, UsernameTakenError } from "./AccountStore.js";

export class ClipRoamStore {
  readonly #accounts: AccountStore;
  readonly #userStores = new Map<string, UserDataStore>();

  constructor(databasePath?: string) {
    this.#accounts = new AccountStore(databasePath);
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
    return this.#userStore(userId).listByIds(entryIds);
  }
  listManifest(userId: string): ClipboardManifestEntry[] { return this.#userStore(userId).listManifest(); }
  upsertDevice(userId: string, device: Device): void { this.#userStore(userId).upsertDevice(device); }
  listDevices(userId: string): Device[] { return this.#userStore(userId).listDevices(); }
  upsert(userId: string, entry: ClipboardEntry): ClipboardEntry {
    return this.#userStore(userId).upsert(entry);
  }
  entryIdForClientId(userId: string, clientId: string): string | undefined {
    return this.#userStore(userId).entryIdForClientId(clientId);
  }
  delete(userId: string, entryId: string): void { this.#userStore(userId).delete(entryId); }
  filePath(userId: string, fileId: string): string { return this.#userStore(userId).filePath(fileId); }
  prepareFilePath(userId: string, fileId: string): string {
    return this.#userStore(userId).prepareFilePath(fileId);
  }
  storeFile(userId: string, fileId: string, size: number, mime?: string): void {
    this.#userStore(userId).storeFile(fileId, size, mime);
  }
  hasFile(userId: string, fileId: string): boolean { return this.#userStore(userId).hasFile(fileId); }
  getFile(userId: string, fileId: string): { path: string; size: number } | undefined {
    return this.#userStore(userId).getFile(fileId);
  }
  collectGarbage(userId: string, partialTtlMs: number): { removedFiles: number; removedBytes: number } {
    return this.#userStore(userId).collectGarbage(partialTtlMs);
  }
  loadedUserIds(): string[] { return [...this.#userStores.keys()]; }

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
