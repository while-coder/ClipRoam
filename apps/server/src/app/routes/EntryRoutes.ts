import type { FastifyInstance } from "fastify";
import {
  ENTRY_PAGE_DEFAULT_LIMIT,
  MAX_MESSAGE_BYTES,
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
import type { ClipRoamStore } from "../../storage/ClipRoamStore.js";
import { requireSessionUser } from "./SessionUser.js";

const logger = getLogger("EntryRoutes");

export type EntryRouteDeps = {
  store: Pick<ClipRoamStore, "listManifestPage" | "listByIds" | "upsert" | "delete">;
  broadcast: (userId: string, message: ServerMessage, exceptDeviceId?: string) => void;
};

// Entries are pure request/response over HTTP; the WebSocket only carries
// server-push notifications about them. The publish response is the
// sender's confirmation, and the `clipboard.created` push still reaches the
// publisher: its local write is an idempotent upsert, and the push clears
// any pending metadata-update mark from the same event.
export function registerEntryRoutes(app: FastifyInstance, deps: EntryRouteDeps): void {
  const { store, broadcast } = deps;

  // Paginated identity listing with optional keyword and UTC date-range
  // filters. Ids only, so a page stays small; details arrive through
  // POST /entries/query. It doubles as the connection-time reconciliation
  // snapshot: an unfiltered walk until hasMore turns false covers the account.
  app.get("/entries/manifest", async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const parsed = EntryManifestQuerySchema.safeParse(request.query);
    if (!parsed.success) return reply.code(400).send({ message: "查询参数无效" });
    return store.listManifestPage(user.id, parsed.data, ENTRY_PAGE_DEFAULT_LIMIT) satisfies EntryManifestResponse;
  });

  app.post("/entries/query", { bodyLimit: 64 * 1024 }, async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const parsed = EntryQueryRequestSchema.safeParse(request.body);
    if (!parsed.success) return reply.code(400).send({ message: "查询参数无效" });
    return { entries: store.listByIds(user.id, parsed.data.entryIds) } satisfies EntryQueryResponse;
  });

  // Entries carry an unbounded directory tree, so the body limit matches the
  // WebSocket maxPayload the publish message used to travel within.
  app.post("/entries", { bodyLimit: MAX_MESSAGE_BYTES }, async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const parsed = EntryPublishRequestSchema.safeParse(request.body);
    if (!parsed.success) return reply.code(400).send({ message: "剪贴板参数无效" });
    const storedEntry = store.upsert(user.id, {
      ...parsed.data.entry,
      sourceDeviceId: parsed.data.deviceId,
    });
    logger.info(`Clipboard entry stored: user=${user.id} entry=${storedEntry.id} device=${parsed.data.deviceId}`);
    // The response is the publisher's confirmation; the push below still
    // reaches the publisher, whose local write is an idempotent upsert.
    broadcast(user.id, { type: "clipboard.created", entry: storedEntry });
    return { entry: storedEntry } satisfies EntryPublishResponse;
  });

  app.post("/entries/:id/activate", async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const { id } = request.params as { id: string };
    const [entry] = store.listByIds(user.id, [id]);
    if (!entry) return reply.code(404).send({ message: "剪贴板记录不存在" });
    const parsed = EntryActivateRequestSchema.safeParse(request.body ?? {});
    if (!parsed.success) return reply.code(400).send({ message: "激活参数无效" });
    // File-list clipboards are intentionally history-only. Broadcasting them
    // would make receivers materialize unused directory views and temporary
    // files before the user has chosen to paste anything. The 200 response
    // still carries the entry, so "stored but not broadcast" stays
    // distinguishable from "not found".
    if (entry.kind !== "files") {
      // Self-excluded on purpose: a delayed self-echo would overwrite a
      // newer local clipboard captured moments after this one.
      broadcast(user.id, { type: "clipboard.activated", entry }, parsed.data.deviceId);
      logger.info(`Clipboard activated: user=${user.id} entry=${entry.id} device=${parsed.data.deviceId}`);
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
