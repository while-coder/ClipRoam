import { computed, ref } from "vue";
import type { FileRelayRequest } from "@cliproam/protocol";
import type { AccountSession } from "../sync/accountSession";
import type { ServeTaskSnapshot } from "./types";

const SERVE_TASK_LIMIT = 100;

/** Shared relay upload ledger for the active account; the worker only transfers bytes. */
export class UploadManager {
  readonly tasks = ref<ServeTaskSnapshot[]>([]);
  readonly activeCount = computed(() => this.tasks.value.filter((task) => task.status === "pending" || task.status === "serving").length);
  #serveTasks = new Map<string, ServeTaskSnapshot>();
  constructor(private readonly session: AccountSession) {}
  /** 任务台账按新→旧排序的快照；「上传」页整页展示。 */
  snapshot(): readonly ServeTaskSnapshot[] {
    return [...this.#serveTasks.values()].sort((a, b) => b.startedAt - a.startedAt);
  }

  record(request: FileRelayRequest): ServeTaskSnapshot {
    const task: ServeTaskSnapshot = {
      id: request.sessionId,
      entryId: request.entryId,
      fileId: request.fileId,
      label: request.fileId.slice(0, 8),
      size: request.size - (request.offset ?? 0),
      status: "pending",
      sentBytes: 0,
      startedAt: Date.now(),
    };
    this.#serveTasks.set(task.id, task);
    this.#trimServeTasks();
    this.#notifyServeTasks();
    void this.session.history.getEntry(request.entryId)
      .then((entry) => {
        const label = entry.content;
        // The entry may have been trimmed while resolving; only live tasks update.
        if (!this.session.signal.aborted && label && this.#serveTasks.get(task.id) === task) {
          task.label = label;
          this.#notifyServeTasks();
        }
      })
      .catch(() => undefined);
    return task;
  }

  update(task: ServeTaskSnapshot, patch: Partial<ServeTaskSnapshot>): void {
    if (this.#serveTasks.get(task.id) !== task) return;
    Object.assign(task, patch);
    this.#trimServeTasks();
    this.#notifyServeTasks();
  }

  /** 超限时优先淘汰最早的终态任务，活动任务始终保留。 */
  #trimServeTasks(): void {
    if (this.#serveTasks.size <= SERVE_TASK_LIMIT) return;
    const terminal = [...this.#serveTasks.values()]
      .filter((task) => task.status !== "pending" && task.status !== "serving")
      .sort((a, b) => a.startedAt - b.startedAt);
    while (this.#serveTasks.size > SERVE_TASK_LIMIT && terminal.length) {
      this.#serveTasks.delete(terminal.shift()!.id);
    }
  }

  #notifyServeTasks(): void {
    if (!this.session.signal.aborted) this.tasks.value = [...this.snapshot()];
  }
}
