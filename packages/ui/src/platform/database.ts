import type { PendingRow, StoredEntry } from "../types";

/** Browser-owned persistence. Each account has its own database. */
export type StoreName = "entries" | "files" | "pending" | "downloads" | "sources" | "blobs" | "settings" | "captures";
type GlobalStoreName = "meta" | "file_hash";
type Row<T> = { id: string | number; value: T };
const accountStores: StoreName[] = ["entries", "files", "pending", "downloads", "sources", "blobs", "settings", "captures"];
const openings = new Map<string, Promise<IDBDatabase>>();

function database(name: string, stores: readonly string[]): Promise<IDBDatabase> {
  const existing = openings.get(name);
  if (existing) return existing;
  const opening = new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(name, 1);
    let blocked = false;
    request.onupgradeneeded = () => {
      for (const store of stores) request.result.createObjectStore(store, { keyPath: "id", autoIncrement: store === "pending" });
    };
    request.onerror = () => { openings.delete(name); reject(request.error); };
    request.onblocked = () => {
      blocked = true;
      openings.delete(name);
      reject(new Error("请关闭其他 ClipRoam 页面后重试数据库升级"));
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
    super(`cliproam-${account}`, accountStores);
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

export type FileHashRecord = { hash: string; size: number; modifiedAt?: number };
export const applicationDatabase = new LocalDatabase<GlobalStoreName>("cliproam-global", ["meta", "file_hash"]);
