import { FILE_CHUNK_SIZE, entryContents, UploadBeginResponseSchema, UploadChunkResponseSchema, type ClipboardEntry, type UploadBeginRequest, type UploadBeginResponse, type UploadChunkResponse } from "@cliproam/protocol";
import { invoke } from "@tauri-apps/api/core";
import type { SyncRequester } from "../sync/syncHttp";
import { base64ToBytes } from "../../utils/bytes";

const UPLOAD_BEGIN_TIMEOUT_MS = 30_000;
const UPLOAD_CHUNK_TIMEOUT_MS = 120_000;
const TRANSFER_CONCURRENCY = 4;
type FileReference = { fileId: string; size: number };
export type FileUploaderDeps = {
  isStopped(): boolean;
  onUploadProgress(entryId: string, uploadedBytes: number, totalBytes: number): void;
  onUploadFinished(entryId: string): void;
  onFileAvailable(fileId: string): void;
  onError(message: string): void;
};

/** Upload captured content to the server pool as part of one pending row. */
export class FileUploader {
  constructor(private readonly http: SyncRequester, private readonly deps: FileUploaderDeps) {}
  /**
   * Content ids are known before publishing, so a finished upload never changes
   * the entry — the server just learns it now holds those bytes. Everything is
   * uploaded unconditionally: the server's content-addressed store absorbs
   * duplicates, and locally cached availability marks would go stale anyway.
   * The candidates derive from the entry payload itself, so this works for a
   * published row and for a queue row that has no local entry yet alike.
   */
  async uploadEntry(entry: ClipboardEntry, sizeLimit: number): Promise<void> {
    if (entry.kind !== "files" && entry.kind !== "image") return;
    this.#checkStopped();
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
          await this.#uploadContent(file, (fileUploadedBytes) => {
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
      this.#checkStopped();
      const uploaded = results.flatMap((result) => (
        result.status === "fulfilled" ? [result.value] : []
      ));
      // Refresh history so its file-status backfill observes the completed uploads.
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
      this.#checkStopped();
      const begin = await this.#uploadBegin(file);
      this.#checkStopped();
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
        this.#checkStopped();
        // Spread simultaneous devices across the server's currently missing chunks.
        const index = missing[Math.floor(Math.random() * missing.length)]!;
        const offset = index * FILE_CHUNK_SIZE;
        const length = Math.min(FILE_CHUNK_SIZE, file.size - offset);
        const data = await invoke<string>("read_upload_chunk", {
          fileId: file.fileId, offset, length,
        });
        this.#checkStopped();
        if (!data) throw new Error("本机文件内容不可用");
        // A concurrent upload may store the same content mid-transfer; the
        // chunk response then reports `stored` and the remaining bytes are done.
        const chunk = await this.#uploadChunk(file.fileId, index, base64ToBytes(data));
        this.#checkStopped();
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

  #checkStopped(): void {
    if (this.deps.isStopped()) throw new Error("同步已停止");
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

}

function decodeMissing(missing: string, chunkCount: number): number[] {
  const bytes = base64ToBytes(missing);
  const indices: number[] = [];
  for (let index = 0; index < chunkCount; index++) {
    if (bytes[index >> 3]! & (1 << (index & 7))) indices.push(index);
  }
  return indices;
}


/**
 * Runs `worker` over `items` with at most `limit` in flight. Unlike
 * `Promise.allSettled` over the whole list, memory and socket pressure stay
 * bounded no matter how many items there are. Results keep the input order.
 */
async function mapWithConcurrency<T, R>(
  items: readonly T[],
  limit: number,
  worker: (item: T, index: number) => Promise<R>,
): Promise<PromiseSettledResult<R>[]> {
  const results = new Array<PromiseSettledResult<R>>(items.length);
  let next = 0;

  const runner = async (): Promise<void> => {
    for (let index = next++; index < items.length; index = next++) {
      try {
        results[index] = { status: "fulfilled", value: await worker(items[index]!, index) };
      } catch (reason) {
        results[index] = { status: "rejected", reason };
      }
    }
  };

  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, runner));
  return results;
}
