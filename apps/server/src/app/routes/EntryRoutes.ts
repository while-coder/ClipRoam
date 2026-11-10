import type { FastifyInstance } from "fastify";
import {
  MAX_PUBLISH_BYTES,
  EntryCollectionSchema,
  ENTRY_PAGE_DEFAULT_LIMIT,
  HistoryManifestQuerySchema,
  FavoriteManifestQuerySchema,
  EntryPublishRequestSchema,
  EntryQueryRequestSchema,
  EntryDeleteRequestSchema,
  EntryFavoriteRequestSchema,
  EntryStickyRequestSchema,
  FavoriteFolderInputSchema,
  type EntryManifestResponse,
  type EntryPublishResponse,
  type EntryQueryResponse,
  type EntryActivateResponse,
  type ServerMessage,
} from "@cliproam/protocol";
import { getLogger } from "../Logger.js";
import { SMALL_JSON_BODY_LIMIT } from "../ServerConfig.js";
import type { ClipRoamStore } from "../../account/ClipRoamStore.js";
import { requireSessionUser } from "./SessionUser.js";
import { parseOr400 } from "./parseRequest.js";

const logger = getLogger("EntryRoutes");

export type EntryRouteDeps = {
  store: Pick<ClipRoamStore, "listHistoryManifestPage" | "listFavoriteManifestPage" | "listByIds" | "upsert" | "delete" | "pruneHistory" | "setFavorite" | "listSticky" | "setSticky" |
    "listFavoriteFolders" | "saveFavoriteFolder" | "deleteFavoriteFolder">;
  broadcast: (userId: string, message: ServerMessage, exceptDeviceId?: string) => void;
  // Read per request so an admin settings change applies without a restart.
  maxHistoryEntries: (userId: string) => number;
  maxCaptureFileCount: (userId: string) => number;
  maxStickyEntries: (userId: string) => number;
};

// Entries are pure request/response over HTTP; the WebSocket only carries
// server-push notifications about them. The publish response is the
// sender's confirmation, and the `clipboard.created` push still reaches the
// publisher: its local write is an idempotent upsert, and the push clears
// any pending metadata-update mark from the same event.
export function registerEntryRoutes(app: FastifyInstance, deps: EntryRouteDeps): void {
  const { store, broadcast, maxHistoryEntries, maxCaptureFileCount } = deps;

  app.get("/entries/sticky", async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    return { entries: store.listSticky(user.id) };
  });

  app.patch("/entries/sticky", { bodyLimit: SMALL_JSON_BODY_LIMIT }, async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const body = parseOr400(reply, EntryStickyRequestSchema, request.body, "便签参数无效");
    if (!body) return reply;
    const entries = store.setSticky(user.id, body.collection, body.ids, body.isSticky, deps.maxStickyEntries(user.id));
    if (body.isSticky && entries.length) broadcast(user.id, {
      type: "clipboard.updated", entries: entries.map(({ id, version, collection }) => ({ id, version, collection })),
    });
    if (!body.isSticky) for (const entry of entries) {
      broadcast(user.id, { type: "clipboard.deleted", entryId: entry.id });
    }
    return reply.code(204).send();
  });

  // Routes select independent collections; request bodies never select a table.
  // Sticky notes use the full /entries/sticky response for both list and desktop.
  for (const [path, collection] of [
    ["/entries/manifest", "history"],
    ["/favorites/manifest", "favorites"],
  ] as const) {
    app.get(path, async (request, reply) => {
      const user = requireSessionUser(request, reply);
      if (!user) return reply;
      const schema = collection === "favorites" ? FavoriteManifestQuerySchema : HistoryManifestQuerySchema;
      const query = parseOr400(reply, schema, request.query, "查询参数无效");
      if (!query) return reply;
      const limit = query.pageSize ?? ENTRY_PAGE_DEFAULT_LIMIT;
      return (collection === "favorites"
        ? store.listFavoriteManifestPage(user.id, query, limit)
        : store.listHistoryManifestPage(user.id, query, limit)) satisfies EntryManifestResponse;
    });
  }

  for (const [path, collection] of [
    ["/entries", "history"],
    ["/favorites", "favorites"],
    ["/entries/sticky", "sticky"],
  ] as const) {
    app.post(`${path}/query`, { bodyLimit: SMALL_JSON_BODY_LIMIT }, async (request, reply) => {
      const user = requireSessionUser(request, reply);
      if (!user) return reply;
      const body = parseOr400(reply, EntryQueryRequestSchema, request.body, "查询参数无效");
      if (!body) return reply;
      return { entries: store.listByIds(user.id, collection, body.entryIds) } satisfies EntryQueryResponse;
    });

    app.delete(path, { bodyLimit: SMALL_JSON_BODY_LIMIT }, async (request, reply) => {
      const user = requireSessionUser(request, reply);
      if (!user) return reply;
      const body = parseOr400(reply, EntryDeleteRequestSchema, request.body, "删除参数无效");
      if (!body) return reply;
      const ids = [...new Set(body.ids)];
      store.delete(user.id, collection, ids);
      logger.info(`Clipboard entries deleted: user=${user.id} collection=${collection} entries=${ids.join(",")}`);
      // Include already-absent ids so all devices converge on the same state.
      for (const entryId of ids) broadcast(user.id, { type: "clipboard.deleted", entryId });
      return reply.code(204).send();
    });
  }

  // Entries carry an unbounded directory tree, so the publish body needs the
  // protocol-wide cap (MAX_PUBLISH_BYTES).
  app.post("/entries", { bodyLimit: MAX_PUBLISH_BYTES }, async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    if (!fileTreeWithinLimits(request.body, maxCaptureFileCount(user.id))) {
      return reply.code(413).send({ message: "文件数量、目录深度或目录节点数超过服务器上限" });
    }
    const body = parseOr400(reply, EntryPublishRequestSchema, request.body, "剪贴板参数无效");
    if (!body) return reply;
    const storedEntry = store.upsert(user.id, {
      ...body.entry,
      sourceDeviceId: user.deviceId,
    });
    logger.info(`Clipboard entry stored: user=${user.id} entry=${storedEntry.id} device=${user.deviceId}`);
    // The response is the publisher's confirmation; the push below still
    // reaches the publisher, whose local write is an idempotent upsert.
    broadcast(user.id, { type: "clipboard.created", entry: storedEntry });
    // The history cap may have evicted the oldest entries to make room; every
    // device prunes its local copy on this push so it follows the server.
    for (const entryId of store.pruneHistory(user.id, maxHistoryEntries(user.id))) {
      broadcast(user.id, { type: "clipboard.deleted", entryId });
    }
    return { entry: storedEntry } satisfies EntryPublishResponse;
  });

  app.post("/entries/:id/activate", async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const { id } = request.params as { id: string };
    const collection = parseOr400(reply, EntryCollectionSchema.default("history"), (request.query as { collection?: string }).collection, "记录类型无效");
    if (!collection) return reply;
    const [entry] = store.listByIds(user.id, collection, [id]);
    if (!entry) return reply.code(404).send({ message: "剪贴板记录不存在" });
    // File-list clipboards are intentionally history-only. Broadcasting them
    // would make receivers materialize unused directory views and temporary
    // files before the user has chosen to paste anything. The 200 response
    // still carries the entry, so "stored but not broadcast" stays
    // distinguishable from "not found".
    if (entry.kind !== "files") {
      // Self-excluded on purpose: a delayed self-echo would overwrite a
      // newer local clipboard captured moments after this one.
      broadcast(user.id, { type: "clipboard.activated", entry: { id: entry.id, version: entry.version, collection: entry.collection } }, user.deviceId);
      logger.info(`Clipboard activated: user=${user.id} entry=${entry.id} device=${user.deviceId}`);
    }
    return { entry } satisfies EntryActivateResponse;
  });

  app.patch("/entries/favorite", { bodyLimit: SMALL_JSON_BODY_LIMIT }, async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const body = parseOr400(reply, EntryFavoriteRequestSchema, request.body, "收藏参数无效");
    if (!body) return reply;
    const entries = store.setFavorite(user.id, body.collection, body.ids, body.isFavorite, body.favoriteFolderId);
    if (body.isFavorite && entries.length) broadcast(user.id, {
      type: "clipboard.updated", entries: entries.map(({ id, version, collection }) => ({ id, version, collection })),
    });
    if (!body.isFavorite) for (const entry of entries) {
      broadcast(user.id, { type: "clipboard.deleted", entryId: entry.id });
    }
    return reply.code(204).send();
  });

  app.get("/favorites/folders", async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    return { folders: store.listFavoriteFolders(user.id) };
  });

  app.post("/favorites/folders", { bodyLimit: SMALL_JSON_BODY_LIMIT }, async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const body = parseOr400(reply, FavoriteFolderInputSchema, request.body, "目录参数无效");
    if (!body) return reply;
    store.saveFavoriteFolder(user.id, body);
    broadcast(user.id, { type: "favorites.updated" });
    return reply.code(204).send();
  });

  app.patch<{ Params: { id: string } }>("/favorites/folders/:id", { bodyLimit: SMALL_JSON_BODY_LIMIT }, async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const body = parseOr400(reply, FavoriteFolderInputSchema, request.body, "目录参数无效");
    if (!body) return reply;
    store.saveFavoriteFolder(user.id, body, request.params.id);
    broadcast(user.id, { type: "favorites.updated" });
    return reply.code(204).send();
  });

  app.delete<{ Params: { id: string } }>("/favorites/folders/:id", async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    store.deleteFavoriteFolder(user.id, request.params.id);
    broadcast(user.id, { type: "favorites.updated" });
    return reply.code(204).send();
  });

}

function fileTreeWithinLimits(body: unknown, maxFiles: number): boolean {
  const tree = (body as { entry?: { fileInfo?: unknown } } | null)?.entry?.fileInfo;
  if (!tree || typeof tree !== "object") return true;
  const pending = [{ node: tree, depth: 0 }];
  let files = 0;
  let nodes = 0;
  while (pending.length) {
    const { node, depth } = pending.pop()!;
    if (++nodes > 100_000 || depth > 64) return false;
    if (!node || typeof node !== "object") continue;
    if (typeof (node as { f?: unknown }).f === "string") {
      if (++files > maxFiles) return false;
    } else {
      for (const child of Object.values(node)) pending.push({ node: child, depth: depth + 1 });
    }
  }
  return true;
}
