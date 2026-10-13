/** Browser-owned persistence. No Tauri APIs or platform paths belong here. */
export type StoreName = "entries" | "files" | "pending" | "downloads" | "sources" | "blobs" | "meta";
type Row<T> = { account: string; id: string; value: T };
const stores: StoreName[] = ["entries", "files", "pending", "downloads", "sources", "blobs", "meta"];
let opening: Promise<IDBDatabase> | undefined;

function database(): Promise<IDBDatabase> {
  return opening ??= new Promise((resolve, reject) => {
    const request = indexedDB.open("cliproam", 1);
    request.onupgradeneeded = () => {
      for (const name of stores) {
        const store = request.result.createObjectStore(name, { keyPath: ["account", "id"] });
        store.createIndex("account", "account");
      }
    };
    request.onerror = () => { opening = undefined; reject(request.error); };
    request.onblocked = () => { opening = undefined; reject(new Error("请关闭其他 ClipRoam 页面后重试数据库升级")); };
    request.onsuccess = () => {
      request.result.onversionchange = () => { request.result.close(); opening = undefined; };
      resolve(request.result);
    };
  });
}

export class AccountDatabase {
  constructor(readonly account: string) {}

  async get<T>(name: StoreName, id: string): Promise<T | undefined> {
    const db = await database();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction(name, "readonly");
      const request = transaction.objectStore(name).get([this.account, id]);
      transaction.oncomplete = () => resolve((request.result as Row<T> | undefined)?.value);
      transaction.onabort = () => reject(transaction.error);
    });
  }

  async list<T>(name: StoreName): Promise<T[]> {
    const db = await database();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction(name, "readonly");
      const request = transaction.objectStore(name).index("account").getAll(this.account);
      transaction.oncomplete = () => resolve((request.result as Row<T>[]).map((row) => row.value));
      transaction.onabort = () => reject(transaction.error);
    });
  }

  async getMany<T>(name: StoreName, ids: string[]): Promise<(T | undefined)[]> {
    if (!ids.length) return [];
    const db = await database();
    return new Promise((resolve,reject) => {
      const transaction = db.transaction(name,"readonly"), store = transaction.objectStore(name);
      const requests = ids.map((id) => store.get([this.account,id]));
      transaction.oncomplete = () => resolve(requests.map((request) => (request.result as Row<T> | undefined)?.value));
      transaction.onabort = () => reject(transaction.error);
    });
  }

  async ids(name: StoreName): Promise<string[]> {
    const db = await database();
    return new Promise((resolve,reject) => {
      const transaction = db.transaction(name,"readonly");
      const request = transaction.objectStore(name).index("account").getAllKeys(this.account);
      transaction.oncomplete = () => resolve((request.result as [string,string][]).map((key) => key[1]));
      transaction.onabort = () => reject(transaction.error);
    });
  }

  put<T>(name: StoreName, id: string, value: T): Promise<void> {
    return this.batch([{ name, id, value }]);
  }

  delete(name: StoreName, id: string): Promise<void> {
    return this.batch([{ name, id }]);
  }

  async update<T>(name: StoreName, id: string, transform: (previous: T | undefined) => T | undefined): Promise<void> {
    const db = await database();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction(name, "readwrite");
      const store = transaction.objectStore(name);
      const request = store.get([this.account, id]);
      request.onsuccess = () => {
        const value = transform((request.result as Row<T> | undefined)?.value);
        if (value === undefined) store.delete([this.account, id]);
        else store.put({ account: this.account, id, value });
      };
      transaction.oncomplete = () => resolve();
      transaction.onabort = () => reject(transaction.error);
    });
  }

  /** Commit all changes together; callers never observe an incomplete import. */
  async batch(changes: { name: StoreName; id: string; value?: unknown }[]): Promise<void> {
    if (!changes.length) return;
    const db = await database();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction([...new Set(changes.map((change) => change.name))], "readwrite");
      for (const change of changes) {
        const store = transaction.objectStore(change.name);
        if (change.value === undefined) store.delete([this.account, change.id]);
        else store.put({ account: this.account, id: change.id, value: change.value });
      }
      transaction.oncomplete = () => resolve();
      transaction.onabort = () => reject(transaction.error);
    });
  }

  async nextSequence(): Promise<number> {
    const db = await database();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction("meta", "readwrite");
      const store = transaction.objectStore("meta");
      const request = store.get([this.account, "sequence"]);
      let next = 0;
      request.onsuccess = () => {
        next = Math.max(Date.now(), Number(request.result?.value ?? 0) + 1);
        store.put({ account: this.account, id: "sequence", value: next });
      };
      transaction.oncomplete = () => resolve(next);
      transaction.onabort = () => reject(transaction.error);
    });
  }
}

export const applicationDatabase = new AccountDatabase("");
