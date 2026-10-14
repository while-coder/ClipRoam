import { errorMessage, anyAbortSignal, safeName } from "../../utils";
import { computed, ref } from "vue";
import type { LocalClipboardEntry, StoredEntry } from "../../types";
import { entryContents, isFileNode } from "@cliproam/protocol";
import type { AccountSession } from "../../core/accountSession";
import type { SaveTarget } from "../../platform/files";

/** 取消专用哨兵：被取消的批次（含去重合并方连带取消）抛出，调用方据此静默收尾。 */
export class DownloadCancelledError extends Error {}

type DownloadTaskStatus = "queued" | "downloading" | "succeeded" | "failed" | "cancelled";

/** Account-owned download queue snapshot. */
export type DownloadTaskSnapshot = {
  id: string;
  accountKey: string;
  /** 任务创建的批次序号；同一 fileId 的重复任务只统计最新一次。 */
  batchId: number;
  entryId: string;
  fileId: string;
  requestIds: string[];
  /** 面板显示名：entryLabel + 序号，缺省用 fileId 前 8 位。 */
  label: string;
  size: number;
  status: DownloadTaskStatus;
  receivedBytes: number;
  error?: string;
};

export type DownloadRecord = {
  id: string;
  entryId: string;
  label: string;
  size: number;
  fileCount: number;
  status: "queued" | "downloading" | "saving" | "succeeded" | "failed" | "cancelled" | "interrupted";
  createdMs: number;
  finishedMs: number | null;
  receivedBytes: number;
  destination: string;
  error: string | null;
  files: DownloadTaskSnapshot[];
};

export type EntryDownloadToast = {
  id: string;
  entryId: string;
  label: string;
  status: "downloading" | "succeeded" | "failed" | "cancelled";
  total: number;
  totalBytes: number;
  ready: boolean;
  cancelling: boolean;
  error?: string;
  files: Record<string, DownloadTaskSnapshot>;
};

type Task = { snapshot: DownloadTaskSnapshot; controller: AbortController; done: Promise<void>; finish: (error?: unknown) => void };
type Save = { entryId: string; target: SaveTarget; recordId?: string; controller: AbortController; savedCount?: number };
const terminal = (status: string) => ["succeeded", "failed", "cancelled", "interrupted"].includes(status);

/** One account-owned TS queue; no backend worker or second window state. */
export class DownloadManager {
  readonly taskSnapshots = ref<DownloadTaskSnapshot[]>([]);
  readonly storedRecords = ref<DownloadRecord[]>([]);
  readonly entryToasts = ref<EntryDownloadToast[]>([]);
  readonly downloadRecords = computed<DownloadRecord[]>(() => this.storedRecords.value.map((record) => {
    if (record.finishedMs !== null || record.status === "saving") return record;
    const files = this.taskSnapshots.value.filter((task) => task.requestIds.includes(record.id));
    if (!files.length) return record;
    return { ...record, files,
      status: files.every((file) => file.status === "queued") ? "queued" : "downloading",
      receivedBytes: files.reduce((sum, file) => sum + file.receivedBytes, 0),
    };
  }));
  readonly activeCount = computed(() => this.downloadRecords.value.filter((record) => record.finishedMs === null).length);
  #tasks = new Map<string, Task>();
  #queue: Task[] = [];
  #running = false;
  #batch = 0;
  #records = new Map<string, DownloadRecord>();
  #saves = new Map<string, Save>();
  constructor(private readonly session: AccountSession) {}

  async downloadEntry(entryId: string, saveId?: string, retainFailedToast = false): Promise<void> {
    const session = this.session;
    session.signal.throwIfAborted();
    const entry = await session.history.getEntry(entryId);
    session.signal.throwIfAborted();
    // getEntry refreshes previewPath from disk, rather than the history list's stale summary.
    if (!saveId && entry.kind === "image" && entry.summary.previewPath) return;
    const files = entryContents(entry);
    if (!files.length) return;
    const call: EntryDownloadToast = {
      id: crypto.randomUUID(), entryId, label: entry.content, status: "downloading",
      total: files.length, totalBytes: files.reduce((sum, file) => sum + file.size, 0),
      ready: false, cancelling: false, files: {},
    };
    this.entryToasts.value = [...this.entryToasts.value, call];
    const update = (patch: Partial<EntryDownloadToast>) => {
      this.entryToasts.value = this.entryToasts.value.map((item) =>
        item.id === call.id ? { ...item, ...patch } : item);
    };
    try {
      const outcome = await this.batch(entryId, call.id, saveId);
      if (outcome.cancelled) throw new DownloadCancelledError("已取消");
      if (outcome.failedCount > 0) throw new Error(`有 ${outcome.failedCount} 个文件下载失败（共 ${outcome.total} 个）`);
      update({ status: "succeeded" });
    } catch (error) {
      update({ status: error instanceof DownloadCancelledError || session.signal.aborted ? "cancelled" : "failed", error: errorMessage(error) });
      throw error;
    } finally {
      if (!session.signal.aborted) {
        session.history.invalidate();
        const current = this.entryToasts.value.find((item) => item.id === call.id);
        if (!retainFailedToast || current?.status !== "failed") {
          session.schedule(`entry-download-toast-${call.id}`, () => this.dismissEntryToast(call.id), 5000);
        }
      }
    }
  }

  async cancelEntryCall(requestId: string): Promise<void> {
    const session = this.session;
    const call = this.entryToasts.value.find((item) => item.id === requestId);
    if (!call || call.status !== "downloading" || !call.ready || call.cancelling) return;
    this.entryToasts.value = this.entryToasts.value.map((item) => item.id === requestId ? { ...item, cancelling: true } : item);
    try {
      this.cancelRequest(requestId);
    } catch (error) {
      if (!session.signal.aborted) this.entryToasts.value = this.entryToasts.value.map((item) =>
        item.id === requestId && item.status === "downloading" ? { ...item, cancelling: false, error: errorMessage(error) } : item);
    }
  }

  dismissEntryToast(requestId: string): void {
    this.entryToasts.value = this.entryToasts.value.filter((item) => item.id !== requestId || item.status === "downloading");
  }

  async ensureLocalFiles(entry: LocalClipboardEntry, retainFailedToast = false): Promise<LocalClipboardEntry> {
    if (entry.kind !== "files" && entry.kind !== "image") return entry;
    await this.downloadEntry(entry.id, undefined, retainFailedToast);
    return this.session.history.getEntry(entry.id);
  }

  refresh(): void { this.#publish(); }

  async restore(): Promise<void> {
    const partials = (await this.session.database.ids("blobs")).filter((id) => id.startsWith("partial:"));
    await this.session.database.batch(partials.map((id) => ({name:"blobs",id})));
    const records = await this.session.database.list<DownloadRecord>("downloads");
    for (const record of records) {
      let changed = false;
      if (record.finishedMs === null) {
        Object.assign(record, { status: "interrupted", finishedMs: Date.now(), error: "应用退出，下载未完成" });
        changed = true;
      }
      if (!record.destination?.trim()) {
        const entry = await this.session.database.get<StoredEntry>("entries", record.entryId);
        const ids = entry ? entryContents(entry).map((file) => file.fileId) : record.files.map((file) => file.fileId);
        record.destination = await this.session.files.downloadDestination(ids);
        changed = true;
      }
      if (changed) await this.session.database.put("downloads", record.id, record);
      this.#records.set(record.id, record);
    }
    await this.#trim(); this.#publish();
  }

  tasks(): DownloadTaskSnapshot[] { return [...this.#tasks.values()].map((task) => ({ ...task.snapshot, requestIds: [...task.snapshot.requestIds] })); }
  records(): DownloadRecord[] { return [...this.#records.values()].sort((a,b) => b.createdMs - a.createdMs || b.id.localeCompare(a.id)); }

  #publish(): void {
    if (this.session.signal.aborted) return;
    const tasks = this.tasks();
    this.taskSnapshots.value = tasks;
    this.storedRecords.value = this.records();
    this.entryToasts.value = this.entryToasts.value.map((call) => {
      if (call.status !== "downloading") return call;
      const updates = tasks.filter((task) => task.requestIds.includes(call.id));
      return updates.length ? { ...call, ready: true, files: { ...call.files, ...Object.fromEntries(updates.map((task) => [task.fileId, task])) } } : call;
    });
  }

  async #persist(record: DownloadRecord): Promise<void> {
    await this.session.database.put("downloads", record.id, record);
    this.#publish();
  }

  async #trim(): Promise<void> {
    const finished = this.records().filter((record) => record.finishedMs !== null);
    for (let i = 0; i < finished.length; i++) {
      const record = finished[i]!;
      if (i >= 500 || record.finishedMs! < Date.now() - 30 * 86400000) {
        this.#records.delete(record.id);
        await this.session.database.delete("downloads", record.id);
      }
    }
  }

  async batch(entryId: string, requestId: string, saveId?: string): Promise<{ cancelled: boolean; failedCount: number; total: number }> {
    this.session.signal.throwIfAborted();
    const entry = await this.session.history.get(entryId);
    const files = entryContents(entry).filter((file) => file.fileId);
    const record: DownloadRecord = {
      id: requestId, entryId, label: entry.content, size: files.reduce((sum,file) => sum + file.size, 0), fileCount: files.length,
      status: "queued", createdMs: Date.now(), finishedMs: null, receivedBytes: 0, destination: "", error: null, files: [],
    };
    const save = saveId ? this.#saves.get(saveId) : undefined;
    if (saveId && (!save || save.entryId !== entryId)) throw new Error("另存为任务不存在或不匹配");
    if (save) { save.recordId = record.id; record.destination = save.target.value; }
    else record.destination = await this.session.files.downloadDestination(files.map((file) => file.fileId));
    this.#records.set(record.id, record);
    await this.#persist(record);
    await this.#trim();
    const requested: Task[] = [];
    try {
      // Register every requester before draining so batch cancellation is complete.
      for (const file of files) {
        this.session.signal.throwIfAborted();
        const local = await this.session.files.resolve(file.fileId);
        if (record.finishedMs !== null || save?.controller.signal.aborted) {
          this.cancelRequest(requestId); return { cancelled: true, failedCount: 0, total: files.length };
        }
        if (local) continue;
        let task = [...this.#tasks.values()].find((task) => task.snapshot.fileId === file.fileId && !terminal(task.snapshot.status));
        if (!task) {
          let finish!: Task["finish"];
          const done = new Promise<void>((resolve,reject) => { finish = (error) => error ? reject(error) : resolve(); });
          // A queued task can be cancelled before the batch starts awaiting it.
          void done.catch(() => undefined);
          task = { snapshot: { id: crypto.randomUUID(), accountKey: this.session.accountKey, batchId: ++this.#batch,
            entryId, fileId: file.fileId, requestIds: [], label: entry.content, size: file.size, status: "queued", receivedBytes: 0 },
            controller: new AbortController(), done, finish };
          this.#tasks.set(task.snapshot.id, task); this.#queue.push(task);
        }
        task.snapshot.requestIds.push(requestId); requested.push(task);
      }
      record.status = "downloading";
      this.#publish();
      void this.#drain();
      if (save?.target.kind === "browserDirectory") {
        const signal = anyAbortSignal([save.controller.signal, this.session.signal]);
        save.savedCount = await this.session.files.export(entry, save.target, signal, async (fileId) => {
          const task = requested.find((task) => task.snapshot.fileId === fileId);
          if (task) await task.done;
        });
      }
      const results = await Promise.allSettled(requested.map((task) => task.done));
      record.files = requested.map((task) => ({ ...task.snapshot }));
      record.receivedBytes = record.files.reduce((sum,file) => sum + file.receivedBytes, 0);
      const cancelled = String(record.status) === "cancelled" || requested.some((task) => task.snapshot.status === "cancelled") || this.session.signal.aborted;
      const failed = results.filter((result) => result.status === "rejected").length;
      if (record.finishedMs === null) {
        record.status = cancelled ? "cancelled" : failed ? "failed" : save ? "saving" : "succeeded";
        record.error = requested.find((task) => task.snapshot.error)?.snapshot.error ?? null;
        if (!failed && !cancelled) record.receivedBytes = record.size;
        if (record.status !== "saving") record.finishedMs = Date.now();
        await this.#persist(record);
      }
      return { cancelled, failedCount: cancelled ? 0 : failed, total: files.length };
    } catch (error) {
      this.cancelRequest(requestId);
      if (record.finishedMs === null) {
        record.status = this.session.signal.aborted || save?.controller.signal.aborted ? "cancelled" : "failed";
        record.finishedMs = Date.now(); record.error = String(error);
        await this.#persist(record);
      }
      throw error;
    }
  }

  async #drain(): Promise<void> {
    if (this.#running) return;
    this.#running = true;
    try {
      while (this.#queue.length) {
        const task = this.#queue.shift()!;
        if (task.controller.signal.aborted) continue;
        task.snapshot.status = "downloading"; this.#publish();
        try {
          await this.#transfer(task);
          task.snapshot.status = "succeeded"; task.finish();
        } catch (error) {
          task.snapshot.status = task.controller.signal.aborted || this.session.signal.aborted ? "cancelled" : "failed";
          task.snapshot.error = error instanceof Error ? error.message : String(error);
          task.finish(error);
        }
        this.#publish();
        this.session.schedule(`download:${task.snapshot.id}`, () => { this.#tasks.delete(task.snapshot.id); this.#publish(); }, 60000);
      }
    } finally { this.#running = false; }
  }

  async #transfer(task: Task): Promise<void> {
    const http = this.session.http;
    const budget = AbortSignal.timeout(5 * 60000);
    const signal = anyAbortSignal([budget, task.controller.signal, this.session.signal]);
    while (!signal.aborted) {
      const writer = await this.session.files.writer(task.snapshot.fileId, task.snapshot.size);
      try {
        task.snapshot.receivedBytes = 0; this.#publish();
        const response = await http.fetch("GET", `/files/${encodeURIComponent(task.snapshot.entryId)}/${task.snapshot.fileId}`, { signal });
        if (!response.ok) throw new Error(response.status === 401 ? "登录已失效，请重新登录" : `下载请求失败（${response.status}）`);
        if (!response.body) throw new Error("服务器没有返回文件内容");
        const reader = response.body.getReader();
        try {
          for (;;) {
            const { done, value } = await reader.read();
            if (done) break;
            signal.throwIfAborted();
            await writer.append(value);
            task.snapshot.receivedBytes += value.byteLength;
            this.session.schedule("download-progress", () => this.#publish(), 200);
          }
        } finally { await reader.cancel().catch(() => undefined); reader.releaseLock(); }
        signal.throwIfAborted();
        await writer.commit();
        return;
      } catch (error) {
        await writer.abort().catch(() => undefined);
        if (signal.aborted) throw error;
        await new Promise<void>((resolve,reject) => {
          const timer = setTimeout(() => { signal.removeEventListener("abort", cancel); resolve(); }, 3000);
          const cancel = () => { clearTimeout(timer); reject(signal.reason); };
          signal.addEventListener("abort", cancel, { once: true });
        });
      }
    }
    signal.throwIfAborted();
  }

  cancelTask(taskId: string): void {
    const task = this.#tasks.get(taskId);
    if (!task || terminal(task.snapshot.status)) return;
    task.controller.abort(new Error("已取消下载"));
    if (task.snapshot.status === "queued") {
      task.snapshot.status = "cancelled"; task.snapshot.error = "已取消下载"; task.finish(task.controller.signal.reason);
    }
    this.#publish();
  }
  cancelRequest(requestId: string): void { for (const task of this.#tasks.values()) if (task.snapshot.requestIds.includes(requestId)) this.cancelTask(task.snapshot.id); }
  stop(): void { for (const id of this.#tasks.keys()) this.cancelTask(id); for (const save of this.#saves.values()) save.controller.abort(); }

  async cancelRecord(id: string): Promise<void> {
    this.cancelRequest(id);
    for (const [saveId, save] of this.#saves) if (save.recordId === id) await this.cancelSave(saveId);
    const record = this.#records.get(id);
    if (record && record.finishedMs === null) {
      record.status = "cancelled"; record.finishedMs = Date.now(); record.error = "已取消下载"; await this.#persist(record);
    }
  }
  async removeRecords(id?: string): Promise<void> {
    for (const record of this.#records.values()) if (record.finishedMs !== null && (!id || id === record.id)) {
      this.#records.delete(record.id); await this.session.database.delete("downloads", record.id);
    }
    this.#publish();
  }

  async prepareSave(entryId: string): Promise<{ saveId: string } | null> {
    const entry = await this.session.history.get(entryId);
    const nodes = Object.values(entry.fileInfo ?? {});
    const single = entry.kind === "image" || (nodes.length === 1 && isFileNode(nodes[0]!));
    const fileName = entry.kind === "image" ? `${safeName(entry.content)}.webp` : Object.keys(entry.fileInfo ?? {})[0] ?? "files";
    const target = await this.session.files.selectSaveTarget(single,fileName);
    if (!target) return null;
    this.session.signal.throwIfAborted();
    const saveId = crypto.randomUUID(); this.#saves.set(saveId, { entryId, target, controller: new AbortController() });
    return { saveId };
  }
  async cancelSave(id: string): Promise<void> {
    const save = this.#saves.get(id);
    if (!save) return;
    save.controller.abort(new Error("已取消保存")); this.#saves.delete(id);
    const record = save.recordId ? this.#records.get(save.recordId) : undefined;
    if (record && record.finishedMs === null) {
      record.status = "cancelled"; record.finishedMs = Date.now(); record.error = "已取消保存"; await this.#persist(record);
    }
  }
  async finishSave(id: string): Promise<number> {
    const save = this.#saves.get(id);
    if (!save) throw new Error("另存为任务不存在或已结束");
    const record = save.recordId ? this.#records.get(save.recordId) : undefined;
    try {
      const count = save.savedCount ?? await this.session.files.export(await this.session.history.get(save.entryId), save.target, anyAbortSignal([save.controller.signal, this.session.signal]));
      if (record && record.finishedMs === null) {
        record.status = "succeeded"; record.finishedMs = Date.now(); record.receivedBytes = record.size; await this.#persist(record);
      }
      return count;
    } catch (error) {
      if (record && record.finishedMs === null) {
        record.status = save.controller.signal.aborted || this.session.signal.aborted ? "cancelled" : "failed";
        record.finishedMs = Date.now(); record.error = String(error); await this.#persist(record);
      }
      throw error;
    } finally { this.#saves.delete(id); }
  }
}
