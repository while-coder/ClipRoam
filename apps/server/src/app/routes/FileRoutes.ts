import { createReadStream } from "node:fs";
import { PassThrough } from "node:stream";
import type { FastifyInstance } from "fastify";
import {
  FILE_CHUNK_SIZE,
  FileIdSchema,
  FileQueryRequestSchema,
  FileAccessRequestSchema,
  UploadBeginRequestSchema,
} from "@cliproam/protocol";
import type { FileQueryResponse, ServerMessage } from "@cliproam/protocol";
import type { FileRelayService } from "../../files/FileRelayService.js";
import { UploadHttpError, type UploadService } from "../../files/UploadService.js";
import type { ClipRoamStore } from "../../account/ClipRoamStore.js";
import { SMALL_JSON_BODY_LIMIT } from "../ServerConfig.js";
import { requireSessionUser } from "./SessionUser.js";
import { parseOr400 } from "./parseRequest.js";
import { FileAccessTokens, fileContentType, parseFileRange } from "../../files/FileAccess.js";

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
  const accessTokens = new FileAccessTokens();

  app.post("/files/:entryId/:fileId/access", { bodyLimit: SMALL_JSON_BODY_LIMIT }, async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const { entryId, fileId } = request.params as { entryId: string; fileId: string };
    const body = parseOr400(reply, FileAccessRequestSchema, request.body, "文件访问参数无效");
    if (!body) return reply;
    if (!FileIdSchema.safeParse(fileId).success || !store.canReadFile(user.id, entryId, fileId)) {
      return reply.code(404).send({ message: "文件不存在或无权访问" });
    }
    const access = accessTokens.issue(user.id, entryId, fileId, body.name, body.download);
    return { path: `/files/${encodeURIComponent(entryId)}/${fileId}?access=${access}` };
  });

  app.post("/upload/begin", async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const body = parseOr400(reply, UploadBeginRequestSchema, request.body, "上传参数无效");
    if (!body) return reply;
    try {
      return uploads.begin(user.id, body.fileId, body.size, body.kind);
    } catch (error) {
      return uploadError(reply, error);
    }
  });

  app.put("/upload/:fileId", { bodyLimit: FILE_CHUNK_SIZE + 4096 }, async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const { fileId } = request.params as { fileId: string };
    const rawIndex = (request.query as { index?: string }).index;
    const index = Number(rawIndex);
    const chunk = request.body;
    // A session id from a pre-ledger client can never be a content id, so it
    // is rejected here instead of reaching the store. `!rawIndex` keeps an
    // absent/empty index (`Number("") === 0`) from overwriting chunk 0.
    if (!FileIdSchema.safeParse(fileId).success || !rawIndex || !Number.isInteger(index) || index < 0 || !Buffer.isBuffer(chunk)) {
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
    const { entryId, fileId } = request.params as { entryId: string; fileId: string };
    const query = request.query as { access?: string };
    const access = typeof query.access === "string" ? accessTokens.verify(query.access, entryId, fileId) : undefined;
    if (query.access !== undefined && !access) return reply.code(401).send({ message: "文件访问链接已失效" });
    const user = access ? { id: access.userId } : requireSessionUser(request, reply);
    if (!user) return reply;
    if (!FileIdSchema.safeParse(fileId).success || !entryId) {
      return reply.code(400).send({ message: "下载参数无效" });
    }
    if (!store.canReadFile(user.id, entryId, fileId)) {
      return reply.code(404).send({ message: "文件不存在或无权访问" });
    }
    const stored = store.files().get(fileId);
    const size = stored?.size ?? store.files().describe([fileId])[0]?.size ?? 0;
    const etag = `"${fileId}"`;
    const rangeHeader = request.headers.range;
    const useRange = rangeHeader !== undefined && (!request.headers["if-range"] || request.headers["if-range"] === etag);
    const range = useRange ? parseFileRange(rangeHeader, size) : undefined;
    if (range === null) return reply.code(416).header("Content-Range", `bytes */${size}`).send();
    const offset = range?.start ?? 0;
    const end = range?.end ?? size;
    const partial = Boolean(range);
    const headers: Record<string, string> = {
      "Content-Type": access && !access.download ? fileContentType(access.name) : "application/octet-stream",
      "Content-Length": String(end - offset),
      "Accept-Ranges": "bytes",
      ETag: etag,
      "Cache-Control": "private, no-store",
      "Referrer-Policy": "no-referrer",
      "X-Content-Type-Options": "nosniff",
      "Content-Security-Policy": "sandbox; default-src 'none'",
      // Also applies to the hijacked relay response, which bypasses Fastify hooks.
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Expose-Headers": "Content-Range, Content-Disposition, ETag, Accept-Ranges",
    };
    if (access) headers["Content-Disposition"] = `${access.download ? "attachment" : "inline"}; filename="file"; filename*=UTF-8''${encodeURIComponent(access.name).replace(/['()*]/g, char => `%${char.charCodeAt(0).toString(16).toUpperCase()}`)}`;
    if (partial) headers["Content-Range"] = `bytes ${offset}-${end - 1}/${size}`;
    const status = partial ? 206 : 200;
    if (request.method === "HEAD") {
      reply.hijack();
      reply.raw.writeHead(status, headers);
      reply.raw.end();
      return;
    }
    if (offset === size) return reply.code(status).headers(headers).send(Buffer.alloc(0));
    if (stored) {
      return reply.code(status).headers(headers).send(createReadStream(stored.path, { start: offset, end: end - 1 }));
    }
    // Park the requester: the response body is a live pipe fed by whichever
    // device streams the bytes into the session. The response is hijacked so
    // the headers flush immediately and Fastify stays out of the byte path —
    // this connection may stay open for minutes.
    const stream = new PassThrough();
    const session = relays.create(user.id, stream);
    reply.hijack();
    reply.raw.writeHead(status, headers);
    // Flush now so the client sees the parked GET before the first piped byte.
    reply.raw.flushHeaders();
    stream.pipe(reply.raw);
    // A session torn down server-side (idle prune, abandoned pipe) destroys the
    // PassThrough, and destroying a source does not end a piped destination —
    // without this the hijacked response would hang forever and the client
    // would never reach its "retry from another device" path.
    stream.on("close", () => reply.raw.end());
    // The response's close (not the request's — an empty-body GET "closes" as
    // soon as it is fully received) means the requester hung up.
    reply.raw.on("close", () => relays.abandon(session.id));
    broadcast(user.id, {
      type: "file.requested",
      sessionId: session.id,
      fileId,
      entryId,
      size,
      offset,
      endOffset: end,
    });
  });


  // Availability is global: content hashes are trusted for deduplication.
  app.post("/files/query", { bodyLimit: SMALL_JSON_BODY_LIMIT }, async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const body = parseOr400(reply, FileQueryRequestSchema, request.body, "查询参数无效");
    if (!body) return reply;
    return { files: store.files().describe(body.fileIds) } satisfies FileQueryResponse;
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
    // Only the first PUT claims, and the claim binds the session to that
    // sender's device: every device holding the content sees `file.requested`,
    // so without the binding a second sender's chunks would interleave with
    // the winner's and corrupt the byte stream. A claim that fails here means
    // another device won the race.
    if (!relays.admit(sessionId, user.deviceId)) {
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
