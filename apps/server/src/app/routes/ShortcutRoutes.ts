import type { FastifyInstance } from "fastify";
import { MAX_PUBLISH_BYTES, ShortcutTextRequestSchema, type EntryPublishResponse, type ServerMessage } from "@cliproam/protocol";
import type { ClipRoamStore } from "../../account/ClipRoamStore.js";
import { readBearerToken } from "./AuthRoutes.js";
import { requireSessionUser } from "./SessionUser.js";
import { parseOr400 } from "./parseRequest.js";
import { receiveShortcutUpload } from "../../files/ShortcutUploadService.js";
import { UploadHttpError, type UploadService } from "../../files/UploadService.js";
import type { ServerConfig } from "../ServerConfig.js";

type ShortcutRouteDeps = {
  store: Pick<ClipRoamStore, "issueUploadToken" | "authenticateUploadToken" | "revokeUploadToken" | "upsert" | "pruneHistory">;
  broadcast: (userId: string, message: ServerMessage) => void;
  maxHistoryEntries: (userId: string) => number;
  configForUser: (userId: string) => ServerConfig;
  uploads: Pick<UploadService, "begin" | "uploadPart">;
};

export function registerShortcutRoutes(app: FastifyInstance, { store, broadcast, maxHistoryEntries, configForUser, uploads }: ShortcutRouteDeps): void {
  // Only a full login session may issue a token; the response must not be cached.
  app.post("/shortcuts/token", async (request, reply) => {
    reply.header("Cache-Control", "no-store");
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    return store.issueUploadToken(user.id, user.deviceId);
  });

  app.delete("/shortcuts/token", async (request, reply) => {
    const token = readBearerToken(request.headers.authorization);
    const user = request.sessionUser ?? (token ? store.authenticateUploadToken(token) : undefined);
    if (!user) return reply.code(401).send({ message: "上传凭证已失效" });
    store.revokeUploadToken(user.id, user.deviceId);
    return reply.code(204).send();
  });

  // Restricted credentials are accepted only by these upload endpoints.
  app.post("/shortcuts/text", { bodyLimit: MAX_PUBLISH_BYTES }, async (request, reply) => {
    const token = readBearerToken(request.headers.authorization);
    const user = token ? store.authenticateUploadToken(token) : undefined;
    if (!user) return reply.code(401).send({ message: "上传凭证已失效，请打开 ClipRoam 重新配置" });
    const body = parseOr400(reply, ShortcutTextRequestSchema, request.body, "文本参数无效");
    if (!body) return reply;
    const entry = store.upsert(user.id, { kind: "text", content: body.text, sourceDeviceId: user.deviceId });
    broadcast(user.id, { type: "clipboard.created", entry });
    for (const entryId of store.pruneHistory(user.id, maxHistoryEntries(user.id))) {
      broadcast(user.id, { type: "clipboard.deleted", entryId });
    }
    return { entry: { id: entry.id } } satisfies EntryPublishResponse;
  });

  app.post("/shortcuts/files", async (request, reply) => {
    const token = readBearerToken(request.headers.authorization);
    const user = token ? store.authenticateUploadToken(token) : undefined;
    if (!user) return reply.code(401).send({ message: "上传凭证已失效，请打开 ClipRoam 重新配置" });
    try {
      const input = await receiveShortcutUpload(request, user.id, configForUser(user.id), uploads);
      const entry = store.upsert(user.id, { ...input, sourceDeviceId: user.deviceId });
      broadcast(user.id, { type: "clipboard.created", entry });
      for (const entryId of store.pruneHistory(user.id, maxHistoryEntries(user.id))) {
        broadcast(user.id, { type: "clipboard.deleted", entryId });
      }
      return { entry: { id: entry.id } } satisfies EntryPublishResponse;
    } catch (error) {
      if (error instanceof UploadHttpError) return reply.code(error.status).send({ message: error.message });
      throw error;
    }
  });
}
