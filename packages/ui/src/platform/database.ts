import { appName } from "../branding";
import { profileName } from "../utils";
import type { PendingRow, StoredEntry } from "../types";

/** Browser-owned persistence. Each account has its own database. */
export type StoreName = "entries" | "favorites" | "files" | "pending" | "downloads" | "blobs" | "settings" | "captures";
type Row<T> = { id: string | number; value: T };
const accountStores: StoreName[] = ["entries", "favorites", "files", "pending", "downloads", "blobs", "settings", "captures"];
const openings = new Map<string, Promise<IDBDatabase>>();

function database(name: string, stores: readonly string[]): Promise<IDBDatabase> {
  const existing = openings.get(name);
  if (existing) return existing;
  const opening = new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(name, 3);
    let blocked = false;
    request.onupgradeneeded = (event) => {
      const db = request.result;
      for (const store of stores) {
        if (!db.objectStoreNames.contains(store)) db.createObjectStore(store, { keyPath: "id", autoIncrement: store === "pending" });
      }
      if (name === "cliproam-global") {
        if (event.oldVersion < 2 && db.objectStoreNames.contains("file_hash")) db.deleteObjectStore("file_hash");
        const hashes = db.objectStoreNames.contains("file_hash")
          ? request.transaction!.objectStore("file_hash")
          : db.createObjectStore("file_hash", { keyPath: "path" });
        hashes.createIndex("hash", "hash", { unique: false });
      } else if (db.objectStoreNames.contains("sources")) {
        db.deleteObjectStore("sources");
      }
    };
    request.onerror = () => { openings.delete(name); reject(request.error); };
    request.onblocked = () => {
      blocked = true;
      openings.delete(name);
      reject(new Error(`请关闭其他 ${appName.value} 页面后重试数据库升级`));
    };
    request.onsuccess = () => {
      const db = request.result;
      if (blocked) { db.close(); return; }
      db.onversionchange = () => { db.close(); openings.delete(name); };
      resolve(db);
    };
  });
  openings.set(name, opening);
  return opening;
}

class LocalDatabase<S extends string> {
  constructor(readonly name: string, private readonly stores: readonly S[]) {}

  protected open(): Promise<IDBDatabase> { return database(this.name, this.stores); }

  async get<T>(name: S, id: string | number): Promise<T | undefined> {
    const db = await this.open();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction(name, "readonly");
      const request = transaction.objectStore(name).get(id);
      transaction.oncomplete = () => resolve((request.result as Row<T> | undefined)?.value);
      transaction.onabort = () => reject(transaction.error);
    });
  }

  async list<T>(name: S): Promise<T[]> {
    const db = await this.open();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction(name, "readonly");
      const request = transaction.objectStore(name).getAll();
      transaction.oncomplete = () => resolve((request.result as Row<T>[]).map((row) => row.value));
      transaction.onabort = () => reject(transaction.error);
    });
  }

  async getMany<T>(name: S, ids: (string | number)[]): Promise<(T | undefined)[]> {
    if (!ids.length) return [];
    const db = await this.open();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction(name, "readonly"), store = transaction.objectStore(name);
      const requests = ids.map((id) => store.get(id));
      transaction.oncomplete = () => resolve(requests.map((request) => (request.result as Row<T> | undefined)?.value));
      transaction.onabort = () => reject(transaction.error);
    });
  }

  async ids(name: S): Promise<string[]> {
    const db = await this.open();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction(name, "readonly");
      const request = transaction.objectStore(name).getAllKeys();
      transaction.oncomplete = () => resolve(request.result.map(String));
      transaction.onabort = () => reject(transaction.error);
    });
  }

  put<T>(name: S, id: string | number, value: T): Promise<void> { return this.batch([{ name, id, value }]); }
  delete(name: S, id: string | number): Promise<void> { return this.batch([{ name, id }]); }

  async update<T>(name: S, id: string | number, transform: (previous: T | undefined) => T | undefined): Promise<void> {
    const db = await this.open();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction(name, "readwrite");
      const store = transaction.objectStore(name);
      const request = store.get(id);
      request.onsuccess = () => {
        const value = transform((request.result as Row<T> | undefined)?.value);
        if (value === undefined) store.delete(id);
        else store.put({ id, value });
      };
      transaction.oncomplete = () => resolve();
      transaction.onabort = () => reject(transaction.error);
    });
  }

  /** Commit all changes together; callers never observe an incomplete import. */
  async batch(changes: { name: S; id: string | number; value?: unknown }[]): Promise<void> {
    if (!changes.length) return;
    const db = await this.open();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction([...new Set(changes.map((change) => change.name))], "readwrite");
      try {
        for (const change of changes) {
          const store = transaction.objectStore(change.name);
          if (change.value === undefined) store.delete(change.id);
          else store.put({ id: change.id, value: change.value });
        }
      } catch (error) {
        transaction.abort();
        reject(error);
        return;
      }
      transaction.oncomplete = () => resolve();
      transaction.onabort = () => reject(transaction.error);
    });
  }
}

export class AccountDatabase extends LocalDatabase<StoreName> {
  constructor(readonly account: string) {
    if (!account) throw new Error("账号数据库需要有效的账号标识");
    super(`cliproam-${profileName(account)}`, accountStores);
  }

  /** Deduplication and insertion share one transaction, including across windows. */
  async enqueuePending(entry: StoredEntry, captureId?: string, resolving = false): Promise<number> {
    const db = await this.open();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction(["pending", "captures"], "readwrite");
      const pending = transaction.objectStore("pending"), captures = transaction.objectStore("captures");
      let id = 0;
      const insert = () => {
        const request = pending.add({ value: null });
        request.onsuccess = () => {
          id = request.result as number;
          const row: PendingRow = { id, status: "wait", entry: { ...entry, id: `p${id}` }, captureId, resolving };
          pending.put({ id, value: row });
          if (captureId) captures.put({ id: captureId, value: id });
        };
      };
      if (captureId) {
        const request = captures.get(captureId);
        request.onsuccess = () => {
          if (request.result) id = (request.result as Row<number>).value;
          else insert();
        };
      } else insert();
      transaction.oncomplete = () => resolve(id);
      transaction.onabort = () => reject(transaction.error);
    });
  }
}

export type FileHashRecord = { path: string; hash: string; size: number; modifiedAt?: number };

class ApplicationDatabase extends LocalDatabase<"meta"> {
  constructor() { super("cliproam-global", ["meta"]); }

  async getFileHash(path: string): Promise<FileHashRecord | undefined> {
    const db = await this.open();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction("file_hash", "readonly");
      const request = transaction.objectStore("file_hash").get(path);
      transaction.oncomplete = () => resolve(request.result as FileHashRecord | undefined);
      transaction.onabort = () => reject(transaction.error);
    });
  }

  async findFileHashes(hashes: string[]): Promise<FileHashRecord[]> {
    if (!hashes.length) return [];
    const db = await this.open();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction("file_hash", "readonly");
      const index = transaction.objectStore("file_hash").index("hash");
      const requests = [...new Set(hashes)].map((hash) => index.getAll(hash));
      transaction.oncomplete = () => resolve(requests.flatMap((request) => request.result as FileHashRecord[]));
      transaction.onabort = () => reject(transaction.error);
    });
  }

  async putFileHash(record: FileHashRecord): Promise<void> {
    const db = await this.open();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction("file_hash", "readwrite");
      transaction.objectStore("file_hash").put(record);
      transaction.oncomplete = () => resolve();
      transaction.onabort = () => reject(transaction.error);
    });
  }
}

export const applicationDatabase = new ApplicationDatabase();
