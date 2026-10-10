import { FILE_CHUNK_SIZE, type FileRelayRequest } from "@cliproam/protocol";
import type { AccountSession } from "../sync/accountSession";
import { errorMessageFromBody, type SyncRequester } from "../sync/syncHttp";
import { base64ToBytes } from "../../utils/bytes";
type RelayUploaderDeps = {
  session: AccountSession;
  onServeTasksChanged?: () => void;
  resolveEntryLabel?: (entryId: string) => Promise<string | undefined>;
};
type ServeTaskStatus = "pending" | "serving" | "succeeded" | "skipped" | "failed";

export type ServeTaskSnapshot = {
  id: string;
  entryId: string;
  fileId: string;
  /** 展示名：entry.content；解析不到时用 fileId 前 8 位。 */
  label: string;
  size: number;
  status: ServeTaskStatus;
  sentBytes: number;
  /** 终态补充说明（跳过原因 / 失败原因）。 */
  error?: string;
  startedAt: number;
};

const SERVE_TASK_LIMIT = 100;

/** Each relay request starts immediately; independent sessions run concurrently. */
export class RelayUploader {
  #servingSessions = new Set<string>();
  #serveTasks = new Map<string, ServeTaskSnapshot>();
  constructor(private readonly http: SyncRequester, private readonly deps: RelayUploaderDeps) {}
  // This device may hold the content a `file.requested` push is asking for.
  // Serving it is a loop of local reads streamed as chunked PUTs into the
  // requester's parked relay pipe. Devices without the bytes stay quiet; the
  // server owns relay timeouts; every new request is handled independently.
  serveRelayRequest(request: FileRelayRequest): Promise<void> {
    return this.deps.session.start(() => this.#serveRelayRequest(request));
  }

  async #serveRelayRequest(request: FileRelayRequest): Promise<void> {
    if (this.#servingSessions.has(request.sessionId)) return;
    this.#servingSessions.add(request.sessionId);
    let task: ServeTaskSnapshot | undefined;
    try {
      const startOffset = request.offset ?? 0;
      if (!Number.isSafeInteger(startOffset) || startOffset < 0 || startOffset > request.size) {
        throw new Error("文件起始位置无效");
      }
      // Probe before claiming: a device without the bytes stays quiet so another
      // holder can serve the request. Empty files still need a local existence check.
      const remaining = request.size - startOffset;
      const probe = await this.deps.session.invoke<string>("read_upload_chunk", {
        fileId: request.fileId,
        offset: startOffset,
        length: remaining > 0 ? 1 : 0,
      });
      if (remaining > 0 && !probe) return;
      task = this.#recordServeTask(request);
      this.#updateServeTask(task, { status: "serving" });
      let offset = startOffset;
      for (;;) {
        const length = Math.min(FILE_CHUNK_SIZE, request.size - offset);
        const data = await this.deps.session.invoke<string>("read_upload_chunk", {
          fileId: request.fileId,
          offset,
          length,
        });
        if (length > 0 && !data) throw new Error("本机文件内容不可用");
        const bytes = base64ToBytes(data);
        const last = offset + bytes.byteLength >= request.size;
        const put = await this.http.fetch(
          "PUT",
          `/files/relay/${request.sessionId}${last ? "?end=1" : ""}`,
          {
            headers: { "Content-Type": "application/octet-stream" },
            body: bytes,
          },
        );
        // 410: the requester hung up or the session expired — nothing to serve.
        if (put.status === 410 || put.status === 409) {
          // 409: another device claimed the session first; 410: requester left.
          this.#updateServeTask(task, {
            status: "skipped",
            error: put.status === 409 ? "其他设备已接管" : "中转会话已结束",
          });
          return;
        }
        if (put.status === 401) throw new Error("登录已失效，请重新登录");
        if (!put.ok) {
          const body = await put.json().catch(() => undefined) as unknown;
          throw new Error(errorMessageFromBody(body, put.status));
        }
        offset += bytes.byteLength;
        this.#updateServeTask(task, { sentBytes: offset - startOffset });
        if (last) {
          this.#updateServeTask(task, { status: "succeeded", sentBytes: remaining });
          return;
        }
      }
    } catch (error) {
      if (task) {
        this.#updateServeTask(task, {
          status: "failed",
          error: error instanceof Error ? error.message : String(error),
        });
      }
    } finally {
      this.#servingSessions.delete(request.sessionId);
    }
  }

  /** 任务台账按新→旧排序的快照；「上传」页整页展示。 */
  serveTasksSnapshot(): readonly ServeTaskSnapshot[] {
    return [...this.#serveTasks.values()].sort((a, b) => b.startedAt - a.startedAt);
  }

  #recordServeTask(request: FileRelayRequest): ServeTaskSnapshot {
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
    if (this.deps.resolveEntryLabel) {
      void this.deps.resolveEntryLabel(request.entryId)
        .then((label) => {
          // The entry may have been trimmed while resolving; only live tasks update.
          if (label && this.#serveTasks.get(task.id) === task) {
            task.label = label;
            this.#notifyServeTasks();
          }
        })
        .catch(() => undefined);
    }
    return task;
  }

  #updateServeTask(task: ServeTaskSnapshot, patch: Partial<ServeTaskSnapshot>): void {
    if (this.#serveTasks.get(task.id) !== task) return;
    Object.assign(task, patch);
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
    this.deps.onServeTasksChanged?.();
  }
}

