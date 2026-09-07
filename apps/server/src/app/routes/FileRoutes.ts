import { createReadStream } from "node:fs";
import type { FastifyInstance } from "fastify";
import {
  DOWNLOAD_NOT_STORED_CODE,
  FILE_CHUNK_SIZE,
  FileIdSchema,
  UploadBeginRequestSchema,
} from "@cliproam/protocol";
import type { FileDownloadService } from "../../files/FileDownloadService.js";
import { UploadHttpError, type UploadService } from "../../files/UploadService.js";
import type { ClipRoamStore } from "../../storage/ClipRoamStore.js";

export type FileRouteDeps = {
  sessionUser: (request: { headers: { authorization?: string } }) => { id: string } | undefined;
  uploads: Pick<UploadService, "begin" | "uploadPart">;
  downloads: Pick<FileDownloadService, "request" | "pending" | "waitForRequests">;
  store: Pick<ClipRoamStore, "files" | "canReadFile">;
};

// Content bytes never touch the WebSocket: uploads run as chunked PUTs against
// a preallocated file plus a per-chunk ledger, and downloads stream straight
// from the content pool as raw bytes.
export function registerFileRoutes(app: FastifyInstance, deps: FileRouteDeps): void {
  const { sessionUser, uploads, downloads, store } = deps;

  app.post("/upload/begin", async (request, reply) => {
    const user = sessionUser(request);
    if (!user) return reply.code(401).send({ message: "登录已失效，请重新登录" });
    const parsed = UploadBeginRequestSchema.safeParse(request.body);
    if (!parsed.success) return reply.code(400).send({ message: "上传参数无效" });
    try {
      return uploads.begin(parsed.data.fileId, parsed.data.size);
    } catch (error) {
      return uploadError(reply, error);
    }
  });

  app.put("/upload/:fileId", { bodyLimit: FILE_CHUNK_SIZE + 4096 }, async (request, reply) => {
    const user = sessionUser(request);
    if (!user) return reply.code(401).send({ message: "登录已失效，请重新登录" });
    const { fileId } = request.params as { fileId: string };
    const index = Number((request.query as { index?: string }).index);
    const chunk = request.body;
    // A session id from a pre-ledger client can never be a content id, so it
    // is rejected here instead of reaching the store.
    if (!FileIdSchema.safeParse(fileId).success || !Number.isInteger(index) || index < 0 || !Buffer.isBuffer(chunk)) {
      return reply.code(400).send({ message: "上传参数无效" });
    }
    try {
      return uploads.uploadPart(fileId, index, chunk);
    } catch (error) {
      return uploadError(reply, error);
    }
  });

  // Downloads stream straight from the content pool as raw bytes. When the
  // content is missing, the demand is recorded and the reply fails right
  // away — the client retries while a device holding the bytes serves the
  // demand through the upload routes. The entry scopes the permission
  // check: a content id alone says nothing about who may read it.
  app.get("/files/:entryId/:fileId", async (request, reply) => {
    const user = sessionUser(request);
    if (!user) return reply.code(401).send({ message: "登录已失效，请重新登录" });
    const { entryId, fileId } = request.params as { entryId: string; fileId: string };
    if (!FileIdSchema.safeParse(fileId).success || !entryId) {
      return reply.code(400).send({ message: "下载参数无效" });
    }
    if (!store.canReadFile(user.id, entryId, fileId)) {
      return reply.code(404).send({ message: "文件不存在或无权访问" });
    }
    const stored = store.files().get(fileId);
    if (!stored) {
      downloads.request(
        user.id,
        fileId,
        entryId,
        store.files().describe([fileId])[0]?.size ?? 0,
      );
      return reply.code(404).send({ code: DOWNLOAD_NOT_STORED_CODE, message: "文件尚未同步到服务器" });
    }
    return reply.header("Content-Type", "application/octet-stream")
      .header("Content-Length", String(stored.size))
      .send(createReadStream(stored.path));
  });

  // Devices long-poll here for missing content they might hold. The list is
  // the whole account's demand: whoever actually has the bytes pushes them
  // through the upload routes, so no dedicated source device is needed.
  app.get("/files/requests", async (request, reply) => {
    const user = sessionUser(request);
    if (!user) return reply.code(401).send({ message: "登录已失效，请重新登录" });
    const waitSeconds = clampWait((request.query as { wait?: string }).wait);
    if (waitSeconds > 0 && downloads.pending(user.id).length === 0) {
      await downloads.waitForRequests(user.id, waitSeconds * 1000);
    }
    return { requests: downloads.pending(user.id) };
  });
}

function uploadError(reply: { code: (statusCode: number) => { send: (payload: unknown) => unknown } }, error: unknown): unknown {
  if (error instanceof UploadHttpError) {
    return reply.code(error.status).send({ message: error.message });
  }
  throw error;
}

// The client's long-poll wait in seconds, capped so a hung peer cannot pin a
// server request forever.
function clampWait(value: string | undefined): number {
  const seconds = Number(value);
  if (!Number.isFinite(seconds) || seconds <= 0) return 0;
  return Math.min(Math.floor(seconds), 30);
}
