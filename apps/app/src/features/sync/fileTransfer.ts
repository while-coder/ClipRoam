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
import { downloadStoredFile } from "./fileDownload";
import { errorMessageFromBody, isTransientNetworkError, TRANSIENT_NETWORK_ERROR_MESSAGE, type SyncRequester } from "./syncHttp";

const UPLOAD_BEGIN_TIMEOUT_MS = 30_000;
const UPLOAD_CHUNK_TIMEOUT_MS = 120_000;
const SERVE_RETRY_BACKOFF_MS = 60_000;
// 可恢复上传失败的固定退避：不依赖 socket，退避后直接重新探测 HTTP。
const UPLOAD_RETRY_BACKOFF_MS = 2_000;

export const MANUAL_UPLOAD_LIMIT = 100 * 1024 * 1024;

/** The file-shape fields a download or upload transfer needs. */
export type FileReference = { fileId: string; size: number };

type TransferDeps = {
  isStopped: () => boolean;
  onUploadProgress: (entryId: string, uploadedBytes: number, totalBytes: number) => void;
  onUploadFinished: (entryId: string) => void;
  onFileAvailable: (fileId: string) => void;
  onError: (message: string) => void;
};

/**
 * The file pipeline: chunked resumable uploads, history downloads and
 * `file.requested` relay serving. Everything here is plain HTTP — uploads are
 * content-addressed chunk PUTs driven by the server's ledger bitmap,
 * downloads stream from the pool (parking on a relay pipe for content the
 * server does not hold yet), and serving streams local bytes into a
 * requester's pipe. None of it waits on the socket.
 */
export class FileTransfer {
  #downloadAborts = new Set<AbortController>();
  #servingFiles = new Set<string>();
  #failedServes = new Map<string, number>();
  #entryUploads = new Map<string, Promise<void>>();

  constructor(
    private readonly httpUrl: string,
    private readonly token: string,
    private readonly http: SyncRequester,
    private readonly deps: TransferDeps,
  ) {}

  // Downloads ride HTTP, so they outlive a socket blip — only an explicit
  // stop ends them.
  stop(): void {
    for (const abort of this.#downloadAborts) abort.abort();
    this.#downloadAborts.clear();
  }

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

  async downloadFile(
    entry: ClipboardEntry,
    file: FileReference,
    options: { signal?: AbortSignal } = {},
  ): Promise<void> {
    return this.#downloadFileReference(entry.id, file, undefined, options.signal);
  }

  async downloadFileToSave(
    entry: ClipboardEntry,
    file: FileReference,
    saveId: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<void> {
    return this.#downloadFileReference(entry.id, file, saveId, options.signal);
  }

  async downloadVirtualFile(request: {
    entryId: string;
    fileId: string;
    size: number;
    sourceDeviceId: string;
  }): Promise<void> {
    return this.#downloadFileReference(request.entryId, {
      fileId: request.fileId,
      size: request.size,
    });
  }

  async #downloadFileReference(
    entryId: string,
    file: FileReference,
    saveId?: string,
    signal?: AbortSignal,
  ): Promise<void> {
    const abort = new AbortController();
    // 外部取消（如 UI 取消下载）也走同一中止通道；stop() 仍全局中止。
    if (signal) {
      if (signal.aborted) abort.abort();
      else signal.addEventListener("abort", () => abort.abort(), { once: true });
    }
    this.#downloadAborts.add(abort);
    try {
      // 下载走纯 HTTP + Rust 落盘（公共管线），不依赖 socket；stop() 时才中止。
      await downloadStoredFile(
        { httpUrl: this.httpUrl, token: this.token },
        entryId,
        file,
        { saveId, signal: abort.signal },
      );
    } finally {
      this.#downloadAborts.delete(abort);
    }
  }

  // Downloads pull raw bytes over one HTTP GET: the server streams stored
  // bytes straight off the pool, and content it does not hold parks the
  // request on a live relay pipe that a device holding the bytes fills through
  // `PUT /files/relay/:sessionId`. A pipe that breaks (or its session expires)
  // simply falls back to the next retry — until the deadline. The pipeline
  // itself lives in the shared `fileDownload.ts`, so any window can download
  // with just credentials.

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
    this.#servingFiles.add(request.sessionId);
    try {
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
        if (put.status === 410 || put.status === 409) return;
        if (put.status === 401) throw new Error("登录已失效，请重新登录");
        if (!put.ok) {
          const body = await put.json().catch(() => undefined) as unknown;
          throw new Error(errorMessageFromBody(body, put.status));
        }
        offset += bytes.byteLength;
        if (last) return;
      }
    } catch {
      this.#rememberFailedServe(request.fileId);
    } finally {
      this.#servingFiles.delete(request.sessionId);
    }
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
