import type { FastifyInstance } from "fastify";
import {
  MAX_PUBLISH_BYTES,
  EntryActivateRequestSchema,
  EntryManifestQuerySchema,
  EntryPublishRequestSchema,
  EntryQueryRequestSchema,
  type EntryManifestResponse,
  type EntryPublishResponse,
  type EntryQueryResponse,
  type EntryActivateResponse,
  type ServerMessage,
} from "@cliproam/protocol";
import { getLogger } from "../Logger.js";
import { MANIFEST_PAGE_SIZE, SMALL_JSON_BODY_LIMIT } from "../ServerConfig.js";
import type { ClipRoamStore } from "../../account/ClipRoamStore.js";
import { requireSessionUser } from "./SessionUser.js";
import { parseOr400 } from "./parseRequest.js";

const logger = getLogger("EntryRoutes");

export type EntryRouteDeps = {
  store: Pick<ClipRoamStore, "listManifestPage" | "listByIds" | "upsert" | "delete" | "pruneHistory">;
  broadcast: (userId: string, message: ServerMessage, exceptDeviceId?: string) => void;
  // Read per request so an admin settings change applies without a restart.
  maxHistoryEntries: () => number;
};

// Entries are pure request/response over HTTP; the WebSocket only carries
// server-push notifications about them. The publish response is the
// sender's confirmation, and the `clipboard.created` push still reaches the
// publisher: its local write is an idempotent upsert, and the push clears
// any pending metadata-update mark from the same event.
export function registerEntryRoutes(app: FastifyInstance, deps: EntryRouteDeps): void {
  const { store, broadcast, maxHistoryEntries } = deps;

  // Paginated identity listing with optional keyword, UTC date-range and kind
  // filters. Ids only, so a page stays small; details arrive through
  // POST /entries/query. It doubles as the connection-time reconciliation
  // snapshot: an unfiltered walk until the fetched count reaches total covers
  // the account.
  app.get("/entries/manifest", async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const query = parseOr400(reply, EntryManifestQuerySchema, request.query, "查询参数无效");
    if (!query) return reply;
    // The page size is the client's choice within the schema's bounds;
    // absent, the server default applies.
    const pageSize = query.pageSize ?? MANIFEST_PAGE_SIZE.default;
    return store.listManifestPage(user.id, query, pageSize) satisfies EntryManifestResponse;
  });

  app.post("/entries/query", { bodyLimit: SMALL_JSON_BODY_LIMIT }, async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const body = parseOr400(reply, EntryQueryRequestSchema, request.body, "查询参数无效");
    if (!body) return reply;
    return { entries: store.listByIds(user.id, body.entryIds) } satisfies EntryQueryResponse;
  });

  // Entries carry an unbounded directory tree, so the publish body needs the
  // protocol-wide cap (MAX_PUBLISH_BYTES).
  app.post("/entries", { bodyLimit: MAX_PUBLISH_BYTES }, async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const body = parseOr400(reply, EntryPublishRequestSchema, request.body, "剪贴板参数无效");
    if (!body) return reply;
    const storedEntry = store.upsert(user.id, {
      ...body.entry,
      sourceDeviceId: body.deviceId,
    });
    logger.info(`Clipboard entry stored: user=${user.id} entry=${storedEntry.id} device=${body.deviceId}`);
    // The response is the publisher's confirmation; the push below still
    // reaches the publisher, whose local write is an idempotent upsert.
    broadcast(user.id, { type: "clipboard.created", entry: storedEntry });
    // The history cap may have evicted the oldest entries to make room; every
    // device prunes its local copy on this push so it follows the server.
    for (const entryId of store.pruneHistory(user.id, maxHistoryEntries())) {
      broadcast(user.id, { type: "clipboard.deleted", entryId });
    }
    return { entry: storedEntry } satisfies EntryPublishResponse;
  });

  app.post("/entries/:id/activate", async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const { id } = request.params as { id: string };
    const [entry] = store.listByIds(user.id, [id]);
    if (!entry) return reply.code(404).send({ message: "剪贴板记录不存在" });
    const body = parseOr400(reply, EntryActivateRequestSchema, request.body ?? {}, "激活参数无效");
    if (!body) return reply;
    // File-list clipboards are intentionally history-only. Broadcasting them
    // would make receivers materialize unused directory views and temporary
    // files before the user has chosen to paste anything. The 200 response
    // still carries the entry, so "stored but not broadcast" stays
    // distinguishable from "not found".
    if (entry.kind !== "files") {
      // Self-excluded on purpose: a delayed self-echo would overwrite a
      // newer local clipboard captured moments after this one.
      broadcast(user.id, { type: "clipboard.activated", entry }, body.deviceId);
      logger.info(`Clipboard activated: user=${user.id} entry=${entry.id} device=${body.deviceId}`);
    }
    return { entry } satisfies EntryActivateResponse;
  });

  app.delete("/entries/:id", async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const { id } = request.params as { id: string };
    store.delete(user.id, id);
    logger.info(`Clipboard entry deleted: user=${user.id} entry=${id}`);
    // Broadcast even when the row was already gone: every device cleans up
    // idempotently, and the initiator clears its pending-deletion list.
    broadcast(user.id, { type: "clipboard.deleted", entryId: id });
    return reply.code(204).send();
  });
}
