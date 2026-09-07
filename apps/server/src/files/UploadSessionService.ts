import { createHash, randomUUID, type Hash } from "node:crypto";
import { appendFileSync, closeSync, openSync, readSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { FILE_CHUNK_SIZE, type UploadBeginResponse, type UploadChunkResponse } from "@cliproam/protocol";
import type { ServerConfig } from "../app/ServerConfig.js";
import type { FileStore } from "./FileStore.js";

// An upload failure the HTTP routes translate into a status code verbatim.
export class UploadHttpError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

// The client sent a chunk at an offset the session has already passed past.
export class UploadConflictError extends Error {
  constructor(readonly offset: number) {
    super("上传偏移与服务器进度不一致");
  }
}

type Session = {
  userId: string;
  deviceId: string;
  fileId: string;
  expectedSize: number;
  receivedSize: number;
  hash: Hash;
  temporaryPath: string;
  finalPath: string;
};

// Upload sessions live behind HTTP routes. They are keyed by a server-issued
// id, while the partial bytes on disk stay scoped by account, device, and
// content — so a repeated `begin` (retry, restart, racing twin) reuses the same
// partial file instead of duplicating or corrupting it. Sessions are not bound
// to any socket: a dropped connection never interrupts an upload, and abandoned
// sessions simply expire with their `.part` files under the store's sweep.
export class UploadSessionService {
  #sessions = new Map<string, Session>();

  constructor(
    private readonly files: FileStore,
    private readonly config: ServerConfig,
    private readonly fileAvailable: (userId: string, fileId: string) => void,
  ) {}

  begin(userId: string, deviceId: string, fileId: string, size: number): UploadBeginResponse {
    if (size >= this.config.maxStoredFileBytes) throw new UploadHttpError(413, "文件超过服务器存储上限");
    // Content the server already holds needs no transfer at all.
    if (this.files.has(fileId)) {
      this.fileAvailable(userId, fileId);
      return { status: "stored", fileId };
    }

    // A live session for the same account, device, and content is the same
    // upload — hand it out again with its current progress. A size mismatch
    // cannot happen for genuinely identical content, and a partial file that
    // no longer matches the session (the sweep may have retired it) cannot be
    // trusted either, so both retire the session and start over.
    for (const [id, session] of this.#sessions) {
      if (session.userId !== userId || session.deviceId !== deviceId || session.fileId !== fileId) continue;
      if (session.expectedSize !== size || !this.#partialIntact(session)) {
        this.#sessions.delete(id);
        continue;
      }
      return { status: "ready", sessionId: id, offset: session.receivedSize };
    }

    const finalPath = this.files.preparePath(fileId);
    // Hashing the scope (rather than trusting deviceId verbatim) keeps a
    // client-controlled string out of filesystem paths.
    const temporaryPath = join(
      dirname(finalPath),
      `${createHash("sha256").update(`${userId}:${deviceId}:${fileId}`).digest("hex")}.part`,
    );
    const receivedSize = this.#preparePartialFile(temporaryPath, size);
    const sessionId = randomUUID();
    this.#sessions.set(sessionId, {
      userId,
      deviceId,
      fileId,
      expectedSize: size,
      receivedSize,
      // Feeding the resumed bytes back in keeps the digest incremental, since a
      // hash state cannot be persisted across processes.
      hash: hashPrefix(temporaryPath, receivedSize),
      temporaryPath,
      finalPath,
    });
    return { status: "ready", sessionId, offset: receivedSize };
  }

  append(userId: string, sessionId: string, offset: number, chunk: Buffer): UploadChunkResponse {
    const session = this.#owned(userId, sessionId);
    if (chunk.length > FILE_CHUNK_SIZE) throw new UploadHttpError(400, "文件分块超过限制");
    // Another uploader may have stored this content while we were receiving;
    // every remaining byte is redundant, so settle early.
    if (this.files.has(session.fileId)) {
      this.#settleStored(session, sessionId);
      return { status: "stored", fileId: session.fileId };
    }
    if (offset !== session.receivedSize) throw new UploadConflictError(session.receivedSize);
    if (session.receivedSize + chunk.length > session.expectedSize) {
      throw new UploadHttpError(400, "上传内容超过声明大小");
    }
    appendFileSync(session.temporaryPath, chunk);
    session.hash.update(chunk);
    session.receivedSize += chunk.length;
    return { status: "accepted", received: session.receivedSize };
  }

  complete(userId: string, sessionId: string): { fileId: string } {
    const session = this.#owned(userId, sessionId);
    // The last chunk may have raced a concurrent upload that stored the same
    // content; keeping the already-stored bytes wins either way.
    if (this.files.has(session.fileId)) {
      this.#settleStored(session, sessionId);
      return { fileId: session.fileId };
    }
    if (session.receivedSize !== session.expectedSize) {
      throw new UploadHttpError(400, "上传内容大小不完整");
    }
    // The declared hash decides where the content lands, so accepting bytes that
    // do not match it would let one upload poison every reference to that hash.
    if (session.hash.digest("hex") !== session.fileId) {
      rmSync(session.temporaryPath, { force: true });
      this.#sessions.delete(sessionId);
      throw new UploadHttpError(422, "文件内容校验失败");
    }
    renameSync(session.temporaryPath, session.finalPath);
    this.files.store(session.fileId, session.expectedSize);
    this.#sessions.delete(sessionId);
    this.fileAvailable(session.userId, session.fileId);
    return { fileId: session.fileId };
  }

  // Explicit abort keeps the partial file: a later `begin` for the same content
  // resumes from it, and the sweep retires it if no one ever comes back.
  abort(userId: string, sessionId: string): void {
    this.#owned(userId, sessionId);
    this.#sessions.delete(sessionId);
  }

  #owned(userId: string, sessionId: string): Session {
    const session = this.#sessions.get(sessionId);
    if (!session || session.userId !== userId) throw new UploadHttpError(404, "上传会话不存在或已过期");
    return session;
  }

  // The content is already in the pool, so this session's partial bytes are
  // redundant: drop them and report success.
  #settleStored(session: Session, sessionId: string): void {
    rmSync(session.temporaryPath, { force: true });
    this.#sessions.delete(sessionId);
    this.fileAvailable(session.userId, session.fileId);
  }

  // The session's ledger says `receivedSize` bytes are on disk; verify the file
  // still agrees before resuming it. A swept, truncated, or deleted partial
  // would otherwise let appends silently produce a gapped final file.
  #partialIntact(session: Session): boolean {
    try {
      return statSync(session.temporaryPath).size === session.receivedSize;
    } catch {
      return false;
    }
  }

  #preparePartialFile(temporaryPath: string, expectedSize: number): number {
    try {
      const partial = statSync(temporaryPath);
      const expired = this.config.resumableUploadTtlMs === 0
        || Date.now() - partial.mtimeMs > this.config.resumableUploadTtlMs;
      if (partial.size <= expectedSize && !expired) return partial.size;
      rmSync(temporaryPath, { force: true });
    } catch {
      // A missing partial file simply starts a new upload.
    }
    writeFileSync(temporaryPath, Buffer.alloc(0));
    return 0;
  }
}

function hashPrefix(path: string, byteLength: number): Hash {
  const hash = createHash("sha256");
  if (byteLength <= 0) return hash;
  const descriptor = openSync(path, "r");
  try {
    const buffer = Buffer.allocUnsafe(FILE_CHUNK_SIZE);
    let read = 0;
    while (read < byteLength) {
      const bytes = readSync(descriptor, buffer, 0, Math.min(buffer.length, byteLength - read), read);
      if (bytes <= 0) break;
      hash.update(buffer.subarray(0, bytes));
      read += bytes;
    }
  } finally {
    closeSync(descriptor);
  }
  return hash;
}
