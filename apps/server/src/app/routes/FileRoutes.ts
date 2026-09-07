import { createReadStream } from "node:fs";
import { PassThrough } from "node:stream";
import type { FastifyInstance } from "fastify";
import {
  FILE_CHUNK_SIZE,
  FileIdSchema,
  UploadBeginRequestSchema,
} from "@cliproam/protocol";
import type { ServerMessage } from "@cliproam/protocol";
import type { FileRelayService } from "../../files/FileRelayService.js";
import { UploadHttpError, type UploadService } from "../../files/UploadService.js";
import type { ClipRoamStore } from "../../account/ClipRoamStore.js";
import { requireSessionUser } from "./SessionUser.js";

export type FileRouteDeps = {
  uploads: Pick<UploadService, "begin" | "uploadPart">;
  relays: FileRelayService;
  broadcast: (userId: string, message: ServerMessage) => void;
  store: Pick<ClipRoamStore, "files" | "canReadFile">;
};

// Content bytes never touch the WebSocket: uploads run as chunked PUTs against
// a preallocated file plus a per-chunk ledger, downloads stream straight from
// the content pool as raw bytes, and content the pool does not hold is piped
// online from a device that has it — never landing on the server's disk.
export function registerFileRoutes(app: FastifyInstance, deps: FileRouteDeps): void {
  const { uploads, relays, broadcast, store } = deps;

  app.post("/upload/begin", async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const parsed = UploadBeginRequestSchema.safeParse(request.body);
    if (!parsed.success) return reply.code(400).send({ message: "上传参数无效" });
    try {
      return uploads.begin(parsed.data.fileId, parsed.data.size);
    } catch (error) {
      return uploadError(reply, error);
    }
  });

  app.put("/upload/:fileId", { bodyLimit: FILE_CHUNK_SIZE + 4096 }, async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
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

  // One GET for every download. Content the pool holds streams straight off
  // disk; content it does not hold parks the requester: the response body
  // becomes a live pipe, devices holding the bytes are told via
  // `file.requested`, and the first one streams them through
  // `PUT /files/relay/:sessionId`. Nothing is buffered — when the requester
  // hangs up the pipe is destroyed and the sender's next PUT fails. The entry
  // scopes the permission check: a content id alone says nothing about who
  // may read it.
  app.get("/files/:entryId/:fileId", async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const { entryId, fileId } = request.params as { entryId: string; fileId: string };
    if (!FileIdSchema.safeParse(fileId).success || !entryId) {
      return reply.code(400).send({ message: "下载参数无效" });
    }
    if (!store.canReadFile(user.id, entryId, fileId)) {
      return reply.code(404).send({ message: "文件不存在或无权访问" });
    }
    const stored = store.files().get(fileId);
    if (stored) {
      return reply.header("Content-Type", "application/octet-stream")
        .header("Content-Length", String(stored.size))
        .send(createReadStream(stored.path));
    }
    const size = store.files().describe([fileId])[0]?.size ?? 0;
    // Park the requester: the response body is a live pipe fed by whichever
    // device streams the bytes into the session. The response is hijacked so
    // the headers flush immediately and Fastify stays out of the byte path —
    // this connection may stay open for minutes.
    const stream = new PassThrough();
    const session = relays.create(user.id, entryId, fileId, size, stream);
    reply.hijack();
    reply.raw.writeHead(200, { "Content-Type": "application/octet-stream" });
    // Without a Content-Length the headers would otherwise only flush with
    // the first piped byte; flush now so the client sees the parked GET.
    reply.raw.flushHeaders();
    stream.pipe(reply.raw);
    // The response's close (not the request's — an empty-body GET "closes" as
    // soon as it is fully received) means the requester hung up.
    reply.raw.on("close", () => relays.abandon(session.id));
    broadcast(user.id, {
      type: "file.requested",
      sessionId: session.id,
      fileId,
      entryId,
      size,
    });
  });


  // One chunk of an online relay. The first PUT claims the session (a second
  // sender gets 409), the requester's backpressure gates the response, and a
  // 410 tells the sender the requester is gone. `?end=1` closes the pipe
  // cleanly after the final chunk.
  app.put("/files/relay/:sessionId", { bodyLimit: FILE_CHUNK_SIZE + 4096 }, async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const { sessionId } = request.params as { sessionId: string };
    const chunk = request.body;
    if (!sessionId || !Buffer.isBuffer(chunk)) {
      return reply.code(400).send({ message: "中转参数无效" });
    }
    const session = relays.get(sessionId);
    if (!session || session.stream.destroyed) {
      return reply.code(410).send({ message: "中转会话已结束" });
    }
    if (session.userId !== user.id) {
      return reply.code(404).send({ message: "中转会话不存在" });
    }
    // Only the first PUT claims; later chunks of the same transfer pass
    // through. A claim that fails here means another sender won the race.
    if (!session.claimed && !relays.claim(sessionId)) {
      return reply.code(409).send({ message: "中转会话已被其他设备认领" });
    }
    if (!(await relays.push(sessionId, chunk))) {
      return reply.code(410).send({ message: "请求端已断开" });
    }
    if ((request.query as { end?: string }).end === "1") {
      relays.end(sessionId);
    }
    return reply.code(204).send();
  });
}

function uploadError(reply: { code: (statusCode: number) => { send: (payload: unknown) => unknown } }, error: unknown): unknown {
  if (error instanceof UploadHttpError) {
    return reply.code(error.status).send({ message: error.message });
  }
  throw error;
}
