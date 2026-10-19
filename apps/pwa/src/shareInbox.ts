export type PendingShare = { id: string; text?: string; files: File[]; createdAt: number };

/** Separate from account storage: shares survive login and are removed only after import. */
async function inbox<T>(scope: string, mode: IDBTransactionMode, operation: (store: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const db = await new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(`cliproam-share-inbox:${scope}`, 1);
    request.onupgradeneeded = () => request.result.createObjectStore("shares", {keyPath:"id"});
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  try {
    return await new Promise<T>((resolve, reject) => {
      const transaction = db.transaction("shares", mode);
      const request = operation(transaction.objectStore("shares"));
      transaction.oncomplete = () => resolve(request.result);
      transaction.onabort = () => reject(transaction.error ?? request.error ?? new Error("无法保存系统分享"));
      transaction.onerror = () => reject(transaction.error ?? request.error);
    });
  } finally { db.close(); }
}

export const saveShare = (scope: string, share: PendingShare): Promise<IDBValidKey> =>
  inbox(scope, "readwrite", (store) => store.put(share));
export const listShares = async (scope: string): Promise<PendingShare[]> =>
  (await inbox<PendingShare[]>(scope, "readonly", (store) => store.getAll())).sort((a, b) => a.createdAt - b.createdAt);
export const removeShare = (scope: string, id: string): Promise<undefined> =>
  inbox(scope, "readwrite", (store) => store.delete(id));

export function shareFromForm(form: FormData): PendingShare {
  const parts = ["title", "text", "url"].map((key) => form.get(key)).filter((value): value is string => typeof value === "string" && !!value.trim());
  const files = form.getAll("files").filter((value): value is File => typeof value !== "string" && !!value.name);
  const text = [...new Set(parts)].join("\n");
  if (!text && !files.length) throw new Error("分享内容为空，请重新分享");
  return { id: crypto.randomUUID(), text: text || undefined, files, createdAt: Date.now() };
}
