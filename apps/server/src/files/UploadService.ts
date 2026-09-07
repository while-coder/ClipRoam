import { createHash } from "node:crypto";
import { closeSync, fdatasyncSync, ftruncateSync, openSync, readSync, renameSync, rmSync, statSync, writeSync } from "node:fs";
import { FILE_CHUNK_SIZE, type UploadBeginResponse, type UploadChunkResponse } from "@cliproam/protocol";
import { countWrittenChunks, isBitmapFull, zeroBitmap, type FileStore } from "./FileStore.js";
import type { ServerConfig } from "../app/ServerConfig.js";

// An upload failure the HTTP routes translate into a status code verbatim.
export class UploadHttpError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

type Ledger = { size: number; chunkCount: number; bitmap: Buffer };

// An upload has no session: the state is one preallocated file plus a chunk
// ledger keyed by the content id itself, so any device can resume what another
// left behind and a retry costs nothing. Single process with fully synchronous
// IO is what makes the read-modify-write on the ledger race-free. Chunks may
// arrive in any order — the client picks freely from the `missing` bitmap the
// server answers every request with.
export class UploadService {
  constructor(
    private readonly files: FileStore,
    private readonly config: ServerConfig,
    private readonly fileStored: (fileId: string) => void,
  ) {}

  begin(fileId: string, size: number): UploadBeginResponse {
    // Content the server already holds needs no transfer at all — even when it
    // exceeds a limit that was lowered after it was stored. Availability was
    // broadcast exactly once when the content was promoted, so repeat answers
    // stay silent instead of re-notifying every connected device.
    if (this.files.has(fileId)) {
      return { status: "stored", fileId };
    }
    if (size >= this.config.maxStoredFileBytes) throw new UploadHttpError(413, "文件超过服务器存储上限");
    const chunkCount = Math.ceil(size / FILE_CHUNK_SIZE);
    // An empty content has no chunks: the buffer is complete on arrival, so it
    // is promoted inside this very request.
    if (chunkCount === 0) {
      this.#discard(fileId);
      closeSync(openSync(this.files.partialPath(fileId), "w"));
      this.#promote(fileId, size);
      return { status: "stored", fileId };
    }

    const ledger = this.#usableLedger(fileId, size, chunkCount);
    if (!ledger) {
      this.#startLedger(fileId, size, chunkCount);
      return {
        status: "ready",
        missingChunks: missingFromBitmap(zeroBitmap(chunkCount), chunkCount),
        receivedBytes: 0,
      };
    }
    // A crash after the final bit was committed but before promotion ran leaves
    // a full ledger behind; finishing it here keeps `begin` from handing out an
    // upload with nothing left to send.
    if (isBitmapFull(ledger.bitmap, chunkCount)) {
      this.#promote(fileId, size);
      return { status: "stored", fileId };
    }
    return {
      status: "ready",
      missingChunks: missingFromBitmap(ledger.bitmap, chunkCount),
      receivedBytes: countWrittenChunks(ledger.bitmap) * FILE_CHUNK_SIZE,
    };
  }

  uploadPart(fileId: string, index: number, chunk: Buffer): UploadChunkResponse {
    // Another uploader may have promoted this content between our requests, so
    // the remaining bytes are redundant and the retry settles as a success.
    // This check must precede the ledger lookup: promotion deletes the row.
    if (this.files.has(fileId)) {
      this.#discard(fileId);
      return { status: "stored", fileId };
    }
    const ledger = this.files.uploadLedger(fileId);
    if (!ledger) throw new UploadHttpError(404, "上传不存在或已被清理");
    if (!Number.isInteger(index) || index < 0 || index >= ledger.chunkCount) {
      throw new UploadHttpError(400, "分块序号超出范围");
    }
    // Every chunk but the last is exactly one chunk size; accepting anything
    // else would corrupt the offsets of the chunks around it.
    const length = Math.min(FILE_CHUNK_SIZE, ledger.size - index * FILE_CHUNK_SIZE);
    if (chunk.length !== length) throw new UploadHttpError(400, "文件分块大小不匹配");
    // Resending a chunk already on disk is a no-op: the final hash remains the
    // authority on what the bytes actually are.
    if (ledger.bitmap[index >> 3] & (1 << (index & 7))) return this.#accept(fileId, ledger.bitmap, ledger.chunkCount);

    let descriptor: number;
    try {
      descriptor = openSync(this.files.partialPath(fileId), "r+");
    } catch {
      // The bytes are gone but the ledger row is not — possible only through
      // outside tampering, since reclamation removes both together. The next
      // `begin` finds the mismatch and starts over.
      throw new UploadHttpError(404, "上传不存在或已被清理");
    }
    try {
      // Positioned write into the preallocated file keeps the bytes at exactly
      // the offset the chunk index promises.
      writeSync(descriptor, chunk, 0, length, index * FILE_CHUNK_SIZE);
      // The bytes must survive a power loss before the ledger claims them, or a
      // crash would leave a set bit over a hole in the file.
      fdatasyncSync(descriptor);
    } finally {
      closeSync(descriptor);
    }
    const marked = this.files.markChunkWritten(fileId, index);
    if (!marked) throw new UploadHttpError(404, "上传不存在或已被清理");
    if (marked.full) {
      this.#promote(fileId, ledger.size);
      return { status: "stored", fileId };
    }
    return this.#accept(fileId, marked.bitmap, ledger.chunkCount);
  }

  #accept(fileId: string, bitmap: Buffer, chunkCount: number): UploadChunkResponse {
    return {
      status: "accepted",
      missingChunks: missingFromBitmap(bitmap, chunkCount),
      receivedBytes: countWrittenChunks(bitmap) * FILE_CHUNK_SIZE,
    };
  }

  // Content addressing is the last line of defence: the declared hash decides
  // where the bytes land, so promoting a mismatch would poison every reference
  // to that hash.
  #promote(fileId: string, size: number): void {
    const partialPath = this.files.partialPath(fileId);
    if (hashFile(partialPath) !== fileId) {
      this.#discard(fileId);
      throw new UploadHttpError(422, "文件内容校验失败");
    }
    renameSync(partialPath, this.files.preparePath(fileId));
    this.files.store(fileId, size);
    this.files.removeUploadLedger(fileId);
    this.fileStored(fileId);
  }

  // A ledger that disagrees with the declared size cannot describe the same
  // content, one whose bytes were swept or truncated cannot be trusted, and one
  // past the resumable window is treated as fresh — all three start over.
  #usableLedger(fileId: string, size: number, chunkCount: number): Ledger | undefined {
    const ledger = this.files.uploadLedger(fileId);
    if (!ledger || ledger.size !== size || ledger.chunkCount !== chunkCount) return undefined;
    if (this.config.resumableUploadTtlMs === 0) return undefined;
    try {
      if (Date.now() - statSync(this.files.partialPath(fileId)).mtimeMs > this.config.resumableUploadTtlMs) return undefined;
    } catch {
      return undefined;
    }
    return ledger;
  }

  // Sparse preallocation: holes read as zeros and only chunks whose bit is set
  // are trusted, so disk space is paid for as it is actually written.
  #startLedger(fileId: string, size: number, chunkCount: number): void {
    this.#discard(fileId);
    const descriptor = openSync(this.files.partialPath(fileId), "w");
    try {
      ftruncateSync(descriptor, size);
    } finally {
      closeSync(descriptor);
    }
    this.files.beginUploadLedger(fileId, size, chunkCount);
  }

  #discard(fileId: string): void {
    this.files.removeUploadLedger(fileId);
    rmSync(this.files.partialPath(fileId), { force: true });
  }
}

// Whole-file digest read sequentially: a chunked incremental state cannot be
// persisted across processes, and the assembled file is the only record of the
// bytes that arrived.
function hashFile(path: string): string {
  const hash = createHash("sha256");
  const descriptor = openSync(path, "r");
  try {
    const buffer = Buffer.allocUnsafe(1024 * 1024);
    for (;;) {
      const bytes = readSync(descriptor, buffer, 0, buffer.length, null);
      if (bytes <= 0) break;
      hash.update(buffer.subarray(0, bytes));
    }
  } finally {
    closeSync(descriptor);
  }
  return hash.digest("hex");
}

// The ledger stores "written" bits; the wire carries the complement under the
// name `missingChunks`, with the tail bits past `chunkCount` forced to zero.
function missingFromBitmap(bitmap: Buffer, chunkCount: number): string {
  const missing = Buffer.alloc(bitmap.length);
  const fullBytes = chunkCount >> 3;
  for (let index = 0; index < fullBytes; index++) missing[index] = ~bitmap[index];
  const tailBits = chunkCount & 7;
  if (tailBits !== 0) missing[fullBytes] = ~bitmap[fullBytes] & ((1 << tailBits) - 1);
  return missing.toString("base64");
}
