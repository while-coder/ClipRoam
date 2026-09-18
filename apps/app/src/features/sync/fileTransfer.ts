import {
  FILE_CHUNK_SIZE,
  entryContents,
  UploadBeginResponseSchema,
  UploadChunkResponseSchema,
  type ClipboardEntry,
  type FileRelayRequest,
  type UploadBeginRequest,
  type UploadBeginResponse,
  type UploadChunkResponse,
} from "@cliproam/protocol";
import { invoke } from "@tauri-apps/api/core";

import { mapWithConcurrency, TRANSFER_CONCURRENCY } from "./concurrency";
import { errorMessageFromBody, isTransientNetworkError, TRANSIENT_NETWORK_ERROR_MESSAGE, type SyncRequester } from "./syncHttp";

const UPLOAD_BEGIN_TIMEOUT_MS = 30_000;
const UPLOAD_CHUNK_TIMEOUT_MS = 120_000;
const SERVE_RETRY_BACKOFF_MS = 60_000;
// 可恢复上传失败的固定退避：不依赖 socket，退避后直接重新探测 HTTP。
const UPLOAD_RETRY_BACKOFF_MS = 2_000;

export const MANUAL_UPLOAD_LIMIT = 100 * 1024 * 1024;

/** The file-shape fields a download or upload transfer needs. */
type FileReference = { fileId: string; size: number };

type TransferDeps = {
  isStopped: () => boolean;
  onUploadProgress: (entryId: string, uploadedBytes: number, totalBytes: number) => void;
  onUploadFinished: (entryId: string) => void;
  onFileAvailable: (fileId: string) => void;
  onError: (message: string) => void;
  /** 中继应答任务台账变化（收到 file.requested、进度、终态）后触发。 */
  onServeTasksChanged?: () => void;
  /** 用 entryId 解析展示名（entry.content）；拿不到时回退 fileId 前缀。 */
  resolveEntryLabel?: (entryId: string) => Promise<string | undefined>;
};

// ---------------------------------------------------------------------------
// 中继应答任务（「上传」页数据源）：收到 file.requested 且本机确实能提供内容
// 时记一条任务，随分块 PUT 推进字节数。仅内存台账——应答是短命的临时行为，
// 与跨窗口共享的 Rust Downloader 不同，重连后自然清空。
// ---------------------------------------------------------------------------

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

/**
 * 文件传输管线：分块续传上传 + `file.requested` 中继应答（下载侧的排队/
 * 去重/拉流在 Rust 侧 Downloader，见文件尾的薄桥）。这里全部走普通 HTTP——
 * 上传是按服务器台账位图驱动的 content-addressed 分块 PUT，应答是把本地
 * 字节流灌进请求方的管道。均不等待 socket。
 */
export class FileTransfer {
  #servingFiles = new Set<string>();
  #failedServes = new Map<string, number>();
  #entryUploads = new Map<string, Promise<void>>();
  #serveTasks = new Map<string, ServeTaskSnapshot>();

  constructor(
    private readonly http: SyncRequester,
    private readonly deps: TransferDeps,
  ) {}

  async uploadEntry(entry: ClipboardEntry, sizeLimit: number): Promise<void> {
    const existingUpload = this.#entryUploads.get(entry.id);
    if (existingUpload) return existingUpload;

    const upload = this.#uploadFiles(entry, sizeLimit);
    this.#entryUploads.set(entry.id, upload);
    try {
      await upload;
    } finally {
      this.#entryUploads.delete(entry.id);
    }
  }

  /**
   * Content ids are known before publishing, so a finished upload never changes
   * the entry — the server just learns it now holds those bytes. Everything is
   * uploaded unconditionally: the server's content-addressed store absorbs
   * duplicates, and locally cached availability marks would go stale anyway.
   * The candidates derive from the entry payload itself, so this works for a
   * published row and for a queue row that has no local entry yet alike.
   */
  async #uploadFiles(entry: ClipboardEntry, sizeLimit: number): Promise<void> {
    if (entry.kind !== "files" && entry.kind !== "image") return;
    const candidates = entryContents(entry).filter((file) => file.size < sizeLimit);
    if (!candidates.length) return;

    const totalBytes = candidates.reduce((total, file) => total + file.size, 0);
    const uploadedByFileId = new Map(candidates.map((file) => [file.fileId, 0]));
    this.deps.onUploadProgress(entry.id, 0, totalBytes);
    try {
      const results = await mapWithConcurrency(
        candidates,
        TRANSFER_CONCURRENCY,
        async (file) => {
          await this.#uploadFile(file, (fileUploadedBytes) => {
            uploadedByFileId.set(file.fileId, fileUploadedBytes);
            const uploadedBytes = [...uploadedByFileId.values()].reduce(
              (total, bytes) => total + bytes,
              0,
            );
            this.deps.onUploadProgress(entry.id, uploadedBytes, totalBytes);
          });
          return file.fileId;
        },
      );
      const uploaded = results.flatMap((result) => (
        result.status === "fulfilled" ? [result.value] : []
      ));
      // The server now holds these contents; the UI's live availability set
      // picks this up without waiting for the next pool query.
      for (const fileId of uploaded) {
        this.deps.onFileAvailable(fileId);
      }
      const failures = results.flatMap((result) => (
        result.status === "rejected" ? [result.reason] : []
      ));
      if (failures.length) {
        // A source the local machine can no longer read never becomes
        // uploadable, so that failure keeps the published record (its bytes
        // stay downloadable from the device that still holds them) instead of
        // failing the entry.
        const allMissingSource = failures.every((reason) => (
          String(reason).includes("本机文件内容不可用")
        ));
        if (allMissingSource) {
          this.deps.onError("部分源文件已删除或移动，已保留剪贴板记录和可用文件");
          return;
        }
        // Anything else (auth, network, server) is a real failure: surface it
        // so the caller's retry/skip handling engages instead of leaving the
        // content silently unuploaded.
        throw failures[0];
      }
    } finally {
      this.deps.onUploadFinished(entry.id);
    }
  }

  async #uploadFile(
    file: FileReference,
    onProgress: (uploadedBytes: number) => void,
  ): Promise<void> {
    while (!this.deps.isStopped()) {
      try {
        await this.#uploadContent(file, onProgress);
        return;
      } catch (error) {
        if (this.deps.isStopped() || !isTransientNetworkError(error)) throw error;
        // No socket to wait on: back off and re-probe HTTP directly.
        await new Promise((resolve) => window.setTimeout(resolve, UPLOAD_RETRY_BACKOFF_MS));
      }
    }
    throw new Error(TRANSIENT_NETWORK_ERROR_MESSAGE);
  }

  // Uploads run over HTTP: one POST handshake that either reports the content
  // already stored or hands back the server's chunk ledger, then raw-byte PUTs
  // that each answer with the authoritative ledger. No socket correlation and
  // no ordering constraint — a chunk only needs its index.
  async #uploadContent(
    file: FileReference,
    onProgress: (uploadedBytes: number) => void,
  ): Promise<void> {
    // The ledger can be retired under us — the TTL sweep, or another device's
    // begin discarding state it no longer trusts. A PUT answered with 404 is
    // not a failure: re-beginning hands back the current ledger and the
    // upload continues from it.
    for (let restart = 0; ; restart++) {
      const begin = await this.#uploadBegin(file);
      // The server already had these bytes, so the transfer is over before it
      // began — this is what makes copying a folder twice nearly free.
      if (begin.status === "stored") {
        onProgress(file.size);
        return;
      }
      const chunkCount = Math.ceil(file.size / FILE_CHUNK_SIZE);
      if (begin.receivedBytes > file.size) throw new Error("服务器续传进度超出文件大小");
      // The server may already hold chunks from another device's attempt at
      // the same content, so its bitmap is the only source of truth.
      let missing = decodeMissing(begin.missingChunks, chunkCount);
      onProgress(begin.receivedBytes);
      let retired = false;
      while (missing.length > 0) {
        const index = missing[0]!;
        const offset = index * FILE_CHUNK_SIZE;
        const length = Math.min(FILE_CHUNK_SIZE, file.size - offset);
        const data = await invoke<string>("read_upload_chunk", {
          fileId: file.fileId, offset, length,
        });
        if (!data) throw new Error("本机文件内容不可用");
        // A concurrent upload may store the same content mid-transfer; the
        // chunk response then reports `stored` and the remaining bytes are done.
        const chunk = await this.#uploadChunk(file.fileId, index, base64ToBytes(data));
        if (chunk === undefined) {
          retired = true;
          break;
        }
        if (chunk.status === "stored") {
          onProgress(file.size);
          return;
        }
        const next = decodeMissing(chunk.missingChunks, chunkCount);
        // Bits never clear, so the chunk just sent must have left the ledger;
        // refusing to make progress would loop forever.
        if (next.length >= missing.length) throw new Error("服务器上传进度异常");
        missing = next;
        onProgress(chunk.receivedBytes);
      }
      if (!retired) {
        onProgress(file.size);
        return;
      }
      // A bounded number of restarts keeps a server that keeps answering 404
      // from spinning this loop forever.
      if (restart >= 3) throw new Error("服务器上传进度反复失效");
    }
  }

  async #uploadBegin(file: FileReference): Promise<UploadBeginResponse> {
    const request: UploadBeginRequest = {
      fileId: file.fileId,
      size: file.size,
    };
    const begin = await this.http.request(
      "POST",
      "/upload/begin",
      {
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(request),
        signal: AbortSignal.timeout(UPLOAD_BEGIN_TIMEOUT_MS),
      },
      UploadBeginResponseSchema,
      "服务器返回了不兼容的上传响应",
    );
    return begin!;
  }

  // Returns undefined when the server no longer knows this upload — its
  // ledger was swept or discarded; the caller re-begins to pick up the new one.
  async #uploadChunk(fileId: string, index: number, chunk: Uint8Array): Promise<UploadChunkResponse | undefined> {
    return this.http.request(
      "PUT",
      `/upload/${fileId}?index=${index}`,
      {
        headers: { "Content-Type": "application/octet-stream" },
        body: chunk,
        signal: AbortSignal.timeout(UPLOAD_CHUNK_TIMEOUT_MS),
      },
      UploadChunkResponseSchema,
      "服务器返回了不兼容的上传响应",
      true,
    );
  }

  // Downloads moved to the shared Downloader (Rust transfer/download.rs); this
  // class now owns uploads and relay serving only.

  // This device may hold the content a `file.requested` push is asking for.
  // Serving it is a loop of local reads streamed as chunked PUTs into the
  // requester's parked relay pipe. Devices without the bytes stay quiet; the
  // sender's own failure backoff is keyed by content so a file we cannot
  // provide is not re-probed per session.
  async serveRelayRequest(request: FileRelayRequest): Promise<void> {
    if (this.#servingFiles.has(request.sessionId)) return;
    const failedAt = this.#failedServes.get(request.fileId);
    if (failedAt !== undefined && Date.now() - failedAt < SERVE_RETRY_BACKOFF_MS) return;
    // Probing the first byte first: a device that cannot actually provide the
    // content stays quiet instead of poisoning the session for another holder.
    // 探测失败不记任务——「上传」页只收本机真正待发送的请求。
    if (request.size > 0) {
      const probe = await invoke<string>("read_upload_chunk", {
        fileId: request.fileId,
        offset: 0,
        length: 1,
      });
      if (!probe) {
        this.#rememberFailedServe(request.fileId);
        return;
      }
    }
    const task = this.#recordServeTask(request);
    this.#servingFiles.add(request.sessionId);
    try {
      this.#updateServeTask(task, { status: "serving" });
      let offset = 0;
      for (;;) {
        const length = Math.min(FILE_CHUNK_SIZE, request.size - offset);
        const data = await invoke<string>("read_upload_chunk", {
          fileId: request.fileId,
          offset,
          length,
        });
        if (!data) throw new Error("本机文件内容不可用");
        const bytes = base64ToBytes(data);
        const last = offset + bytes.byteLength >= request.size;
        const put = await this.http.fetch(
          "PUT",
          `/files/relay/${request.sessionId}${last ? "?end=1" : ""}`,
          {
            headers: { "Content-Type": "application/octet-stream" },
            body: bytes,
            signal: AbortSignal.timeout(UPLOAD_CHUNK_TIMEOUT_MS),
          },
        );
        // 410: the requester hung up or the session expired — nothing to serve.
        if (put.status === 410 || put.status === 409) {
          // 409: another device claimed the session first; 410: requester left.
          this.#updateServeTask(task, {
            status: "skipped",
            error: put.status === 409 ? "其他设备已发送" : "请求方已取消",
          });
          return;
        }
        if (put.status === 401) throw new Error("登录已失效，请重新登录");
        if (!put.ok) {
          const body = await put.json().catch(() => undefined) as unknown;
          throw new Error(errorMessageFromBody(body, put.status));
        }
        offset += bytes.byteLength;
        this.#updateServeTask(task, { sentBytes: offset });
        if (last) {
          this.#updateServeTask(task, { status: "succeeded", sentBytes: request.size });
          return;
        }
      }
    } catch (error) {
      this.#updateServeTask(task, {
        status: "failed",
        error: error instanceof Error ? error.message : String(error),
      });
      this.#rememberFailedServe(request.fileId);
    } finally {
      this.#servingFiles.delete(request.sessionId);
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
      size: request.size,
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

  #rememberFailedServe(fileId: string): void {
    this.#failedServes.set(fileId, Date.now());
    if (this.#failedServes.size > 100) {
      const cutoff = Date.now() - SERVE_RETRY_BACKOFF_MS;
      for (const [id, at] of this.#failedServes) {
        if (at < cutoff) this.#failedServes.delete(id);
      }
    }
  }
}

// The Tauri command reads chunks as base64 (its WebSocket-era shape); uploads
// now send raw bytes, so decode before handing them to fetch. The append
// command keeps that base64 signature, so downloads re-encode each read.
function base64ToBytes(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

// Mirrors the server's ledger layout: bit `i` of byte `i >> 3`, least
// significant bit first, one bit per chunk where 1 = still missing. Tail bits
// past `chunkCount` are zero and skipped by the loop bound.
function decodeMissing(missing: string, chunkCount: number): number[] {
  const bytes = base64ToBytes(missing);
  const indices: number[] = [];
  for (let index = 0; index < chunkCount; index++) {
    if (bytes[index >> 3]! & (1 << (index & 7))) indices.push(index);
  }
  return indices;
}

// ---------------------------------------------------------------------------
// 下载薄桥（Downloader）：真正的下载管理在 Rust 侧 transfer/download.rs
// ---------------------------------------------------------------------------
/**
 * 下载管理器（Downloader）的 TS 薄桥：任务表 / FIFO 队列 / 并发上限 / in-flight 去重 /
 * 取消 / 字节进度全部在 Rust 侧全局 Downloader（main 与 paste 两个窗口共享
 * 同一个实例，跨窗口同文件天然合并）。这里只做 invoke 与缓存事件快照——
 * 订阅 onTasksChanged 后重拉 tasksSnapshot() 即可（同 SyncClient 模式）。
 */

/** 取消专用哨兵：被取消的批次（含去重合并方连带取消）抛出，调用方据此静默收尾。 */
export class DownloadCancelledError extends Error {}

type DownloadTaskStatus = "queued" | "downloading" | "succeeded" | "failed" | "cancelled";

/** Rust `cliproam://download-changed` 事件推送的任务快照，字段一一对应。 */
export type DownloadTaskSnapshot = {
  id: string;
  /** 每次 downloadFiles() 自增；同 entry 重复批次按此归代，派生进度只看最新批。 */
  batchId: number;
  entryId: string;
  fileId: string;
  saveId?: string;
  /** 面板显示名：entryLabel + 序号，缺省用 fileId 前 8 位。 */
  label: string;
  size: number;
  status: DownloadTaskStatus;
  receivedBytes: number;
  error?: string;
};

type DownloadRequest = {
  fileId: string;
  size: number;
};

type BatchOutcome = {
  cancelled: boolean;
  failedCount: number;
  total: number;
};

type DownloaderDeps = {
  /** 任务列表变化（Rust 事件推来，含节流后的字节进度）后触发；调用方在此重拉快照。 */
  onTasksChanged?: () => void;
};

export class Downloader {
  #deps: DownloaderDeps;
  #tasks: DownloadTaskSnapshot[] = [];

  constructor(deps: DownloaderDeps) {
    this.#deps = deps;
  }

  /**
   * 一批文件（同一 entry）：任一失败聚合报错，任一取消抛 DownloadCancelledError。
   * 并发与去重在 Rust 侧；webview 中途销毁只影响本次调用，任务照常跑完。
   */
  async downloadFiles(
    entryId: string,
    files: readonly DownloadRequest[],
    options: { saveId?: string; entryLabel?: string } = {},
  ): Promise<void> {
    const outcome = await invoke<BatchOutcome>("download_files", {
      entryId,
      files,
      saveId: options.saveId,
      entryLabel: options.entryLabel,
    });
    if (outcome.cancelled) throw new DownloadCancelledError("已取消");
    if (outcome.failedCount > 0) {
      throw new Error(`有 ${outcome.failedCount} 个文件下载失败（共 ${outcome.total} 个）`);
    }
  }

  cancel(taskId: string): void {
    void invoke("cancel_download", { taskId }).catch(() => undefined);
  }

  /** 取消该 entry 所有非终态任务；返回取消数，0 = 没有下载在跑。 */
  async cancelEntry(entryId: string): Promise<number> {
    return invoke<number>("cancel_entry_downloads", { entryId });
  }

  /** 中止全部活动任务并清空队列（stopSyncClient / 面板「全部取消」共用）。 */
  stopAll(reason?: string): void {
    void invoke("stop_all_downloads", { reason }).catch(() => undefined);
  }

  /** 窗口打开时的初始拉取；之后靠 download-changed 事件跟进。 */
  async refresh(): Promise<void> {
    this.applySnapshot(await invoke<DownloadTaskSnapshot[]>("download_tasks"));
  }

  /** downloading → queued → 终态 排序的任务快照（Rust 已排好序，原样缓存）。 */
  tasksSnapshot(): readonly DownloadTaskSnapshot[] {
    return this.#tasks;
  }

  applySnapshot(payload: DownloadTaskSnapshot[]): void {
    this.#tasks = payload;
    this.#deps.onTasksChanged?.();
  }
}
