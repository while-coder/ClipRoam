import { z } from "zod";

export const DEFAULT_AUTO_UPLOAD_LIMIT = 10 * 1024 * 1024;
export const FILE_CHUNK_SIZE = 128 * 1024;
// A single entry carries its whole directory tree, so a publish body can get
// large. The tree is compact (~40 bytes per node) but unbounded by design.
// The same cap bounds the WebSocket maxPayload.
export const MAX_MESSAGE_BYTES = 16 * 1024 * 1024;
// One HTTP query round-trip covers far more ids than the per-message batches
// the WebSocket era needed; the response size (thumbnails included) is the
// real bound, not the request.
export const ENTRY_QUERY_BATCH = 100;
export const ENTRY_PAGE_DEFAULT_LIMIT = 100;

export const ClipboardKindSchema = z.enum(["text", "files", "image"]);

// A file is addressed by the sha256 of its content, so identical bytes are one
// entity no matter how many entries, paths or devices reference them.
export const FileIdSchema = z.string().regex(/^[0-9a-f]{64}$/);

// A `files` entry is one nested map: a file leaf is `{f, s}` (content id +
// byte size), a directory is another such map, and an empty directory is `{}`.
// Root names are the top-level keys, so roots/dirs/path arrays all disappear.
export const FileNodeSchema = z.object({
  f: FileIdSchema,
  s: z.number().int().nonnegative(),
});
const NodeNameSchema = z.string().min(1).max(255).regex(/^[^/\\]+$/, "名称不能包含路径分隔符");
export const TreeNodeSchema: z.ZodType<TreeNode> = z.lazy(() =>
  z.union([FileNodeSchema, z.record(NodeNameSchema, TreeNodeSchema)]),
);
export const FileInfoSchema = z.record(NodeNameSchema, TreeNodeSchema);

// An `image` entry references one content-pool blob plus its list thumbnail;
// no fake single-file tree is involved.
export const ImageInfoSchema = z.object({
  fileId: FileIdSchema,
  size: z.number().int().nonnegative(),
  thumbnail: z.string().max(96 * 1024),
});

export const ClipboardEntrySchema = z.object({
  id: z.string(),
  kind: ClipboardKindSchema,
  content: z.string(),
  html: z.string().optional(),
  rtf: z.string().optional(),
  // Exactly one of these is present, matching `kind`: the nested file map for
  // `files`, the single blob reference for `image`.
  fileInfo: FileInfoSchema.optional(),
  imageInfo: ImageInfoSchema.optional(),
  sourceDeviceId: z.string(),
  createdAt: z.string(),
  pinned: z.boolean().default(false),
});

// Every content an entry references, de-duplicated in encounter order. Covers
// both kinds, so server-side availability/GC checks have one entry point.
export function entryContents(entry: { kind: string; fileInfo?: FileInfo; imageInfo?: ImageInfo }): Array<{ fileId: string; size: number }> {
  if (entry.kind === "image") {
    return entry.imageInfo ? [{ fileId: entry.imageInfo.fileId, size: entry.imageInfo.size }] : [];
  }
  const contents = new Map<string, number>();
  const walk = (node: TreeNode): void => {
    if (isFileNode(node)) {
      if (!contents.has(node.f)) contents.set(node.f, node.s);
      return;
    }
    for (const child of Object.values(node)) walk(child);
  };
  if (entry.fileInfo) for (const node of Object.values(entry.fileInfo)) walk(node);
  return [...contents].map(([fileId, size]) => ({ fileId, size }));
}

// A leaf holds the content id in `f`; a directory map has no `f` string. A
// directory may legitimately contain a child named "f" — its value is an
// object, not a string, so this check stays unambiguous.
export function isFileNode(node: TreeNode): node is { f: string; s: number } {
  return typeof (node as { f?: unknown }).f === "string";
}

// The connection-time manifest is intentionally small. Full entry metadata is
// fetched only for records missing from the local history.
export const ClipboardManifestEntrySchema = z.object({
  id: z.string(),
});

export const DeviceSchema = z.object({
  id: z.string(),
  name: z.string().min(1).max(80),
  platform: z.string().min(1).max(40),
  osVersion: z.string().min(1).max(80).default("未知"),
});

// `DeviceSchema.id` stays unconstrained on purpose: it validates device
// identity, not length. Request bodies that carry a device id on behalf of the
// sender get this bounded shape instead.
export const DeviceIdSchema = z.string().min(1).max(100);

export const AuthCredentialsSchema = z.object({
  username: z.string().trim().min(3).max(32).regex(/^[a-zA-Z0-9_.-]+$/),
  password: z.string().min(6).max(128),
  deviceId: DeviceIdSchema,
});

export const ChangePasswordSchema = z.object({
  currentPassword: z.string().min(6).max(128),
  newPassword: z.string().min(6).max(128),
}).refine((value) => value.currentPassword !== value.newPassword, {
  path: ["newPassword"],
  message: "新密码不能与当前密码相同",
});

export const AuthResponseSchema = z.object({
  sessionToken: z.string().min(1),
  expiresAt: z.string(),
  user: z.object({
    id: z.string(),
    username: z.string(),
  }),
});

// Entries run over HTTP. The publish response is the sender's confirmation —
// the socket echo the WebSocket flow once waited for no longer exists.
// Identity belongs to the server: it assigns the id (arrival order) and the
// timestamp, and deduplicates by content hash. A client may still send its
// local id and clock time; the server drops both.
export const EntryPublishInputSchema = ClipboardEntrySchema.extend({
  id: z.string().optional(),
  createdAt: z.string().optional(),
});

export const EntryPublishRequestSchema = z.object({
  deviceId: DeviceIdSchema,
  entry: EntryPublishInputSchema,
});

export const EntryPublishResponseSchema = z.object({ entry: ClipboardEntrySchema });

export const EntryQueryRequestSchema = z.object({
  entryIds: z.array(z.string()).min(1).max(ENTRY_QUERY_BATCH),
});

// Ids the server does not know are simply absent, not an error.
export const EntryQueryResponseSchema = z.object({ entries: z.array(ClipboardEntrySchema) });

// Whether the pool holds the bytes for a batch of content ids — the state the
// per-entry `missing` list used to inline. Size rides along so callers can
// render totals for contents the pool has registered but not received yet.
export const FileStatusSchema = z.object({
  fileId: FileIdSchema,
  size: z.number().int().nonnegative(),
  stored: z.boolean(),
});

export const FileQueryRequestSchema = z.object({
  fileIds: z.array(FileIdSchema).min(1).max(ENTRY_QUERY_BATCH),
});

export const FileQueryResponseSchema = z.object({ files: z.array(FileStatusSchema) });

// Offset pagination over entry identities: keyword filter on entry content, an
// inclusive UTC date range, kind and pinned filters, and a 1-based page. Page
// size is the server's choice. Filters apply before paging, so a filtered page
// always holds up to a full page of matching rows.
export const EntryManifestQuerySchema = z.object({
  search: z.string().trim().min(1).max(100).optional(),
  dateStart: z.string().regex(/^\d{4}-\d{2}-\d{2}$/, "dateStart 必须是 YYYY-MM-DD").optional(),
  dateEnd: z.string().regex(/^\d{4}-\d{2}-\d{2}$/, "dateEnd 必须是 YYYY-MM-DD").optional(),
  kind: ClipboardKindSchema.optional(),
  pinned: z.stringbool().optional(),
  page: z.coerce.number().int().min(1).max(100000).optional(),
});

// One page of the identity listing. It doubles as the connection-time
// reconciliation snapshot: a client pages through unfiltered while the
// fetched count is below total. Details arrive through POST /entries/query.
export const EntryManifestResponseSchema = z.object({
  manifest: z.array(ClipboardManifestEntrySchema),
  // Matching rows across all pages (the filters apply before paging), so a
  // client can render "page x of y" and a total without extra requests.
  total: z.number().int().min(0),
});

export const DeviceListResponseSchema = z.object({ devices: z.array(DeviceSchema) });

export const EntryActivateRequestSchema = z.object({ deviceId: DeviceIdSchema });

export const EntryActivateResponseSchema = z.object({ entry: ClipboardEntrySchema });

// The socket only authenticates the connection and carries server-push
// notifications. Every request/response exchange — listing, querying,
// publishing, activating, deleting entries, plus all file bytes and the file
// download orchestration — runs over HTTP.
export const ClientMessageSchema = z.discriminatedUnion("type", [
  z.object({
    type: z.literal("auth"),
    token: z.string().min(1),
    device: DeviceSchema,
  }),
  z.object({ type: z.literal("ping") }),
]);

export const ServerMessageSchema = z.discriminatedUnion("type", [
  // A bare confirmation: the manifest and device list it used to carry ride
  // HTTP instead (see `EntryManifestResponseSchema`).
  z.object({ type: z.literal("auth.ack") }),
  z.object({
    type: z.literal("clipboard.created"),
    entry: ClipboardEntrySchema,
  }),
  z.object({
    type: z.literal("clipboard.activated"),
    entry: ClipboardEntrySchema,
  }),
  z.object({
    type: z.literal("clipboard.deleted"),
    entryId: z.string(),
  }),
  z.object({
    type: z.literal("device.presence"),
    device: DeviceSchema,
    online: z.boolean(),
  }),
  // A file may become available after its entry was already synced to another
  // device. Keep that per-file state current without re-sending the entry.
  z.object({
    type: z.literal("file.available"),
    fileId: FileIdSchema,
  }),
  // A device wants bytes the pool does not hold. The session's held GET is
  // the demand itself: the first device that actually holds the content
  // streams it through `PUT /files/relay/:sessionId`.
  z.object({
    type: z.literal("file.requested"),
    sessionId: z.string().min(1),
    fileId: FileIdSchema,
    entryId: z.string(),
    size: z.number().int().nonnegative(),
  }),
  z.object({ type: z.literal("pong") }),
  z.object({
    type: z.literal("error"),
    code: z.string(),
    message: z.string(),
  }),
]);

// Every status an upload response can carry, in one place. `stored`: the
// server already holds the whole content, nothing left to send. `ready`:
// `begin` is handing out the chunk ledger, start sending. `accepted`: the
// chunk landed but the file is not complete yet, keep going.
export const UploadStatus = {
  stored: "stored",
  ready: "ready",
  accepted: "accepted",
} as const;

// Uploads run over HTTP instead of the WebSocket. There is no session: the
// server tracks an upload as one preallocated file plus a per-chunk ledger, so
// `begin` reports what is already on disk and every PUT answers with the same
// ledger. `missingChunks` is a base64 bitmap over chunk indices where bit 1
// means
// that chunk still has to be sent.
export const UploadBeginRequestSchema = z.object({
  fileId: FileIdSchema,
  size: z.number().int().nonnegative(),
});

export const UploadBeginResponseSchema = z.discriminatedUnion("status", [
  z.object({ status: z.literal(UploadStatus.stored), fileId: FileIdSchema }),
  z.object({
    status: z.literal(UploadStatus.ready),
    missingChunks: z.string(),
    receivedBytes: z.number().int().nonnegative(),
  }),
]);

export const UploadChunkResponseSchema = z.discriminatedUnion("status", [
  z.object({ status: z.literal(UploadStatus.stored), fileId: FileIdSchema }),
  z.object({
    status: z.literal(UploadStatus.accepted),
    missingChunks: z.string(),
    receivedBytes: z.number().int().nonnegative(),
  }),
]);

export type ClipboardKind = z.infer<typeof ClipboardKindSchema>;
// Recursive tree types are written by hand: `z.infer` over `z.lazy` recursion
// struggles to name the recursive reference on its own.
export type TreeNode = { f: string; s: number } | { [name: string]: TreeNode };
export type FileInfo = { [rootName: string]: TreeNode };
export type ImageInfo = z.infer<typeof ImageInfoSchema>;
export type ClipboardEntry = z.infer<typeof ClipboardEntrySchema>;
export type ClipboardManifestEntry = z.infer<typeof ClipboardManifestEntrySchema>;
export type Device = z.infer<typeof DeviceSchema>;
export type AuthCredentials = z.infer<typeof AuthCredentialsSchema>;
export type AuthResponse = z.infer<typeof AuthResponseSchema>;
export type EntryPublishInput = z.infer<typeof EntryPublishInputSchema>;
export type EntryPublishRequest = z.infer<typeof EntryPublishRequestSchema>;
export type EntryPublishResponse = z.infer<typeof EntryPublishResponseSchema>;
export type EntryQueryRequest = z.infer<typeof EntryQueryRequestSchema>;
export type EntryQueryResponse = z.infer<typeof EntryQueryResponseSchema>;
export type FileStatus = z.infer<typeof FileStatusSchema>;
export type FileQueryRequest = z.infer<typeof FileQueryRequestSchema>;
export type FileQueryResponse = z.infer<typeof FileQueryResponseSchema>;
export type EntryActivateResponse = z.infer<typeof EntryActivateResponseSchema>;
export type EntryManifestQuery = z.infer<typeof EntryManifestQuerySchema>;
export type EntryManifestResponse = z.infer<typeof EntryManifestResponseSchema>;
export type DeviceListResponse = z.infer<typeof DeviceListResponseSchema>;
export type EntryActivateRequest = z.infer<typeof EntryActivateRequestSchema>;
export type UploadBeginRequest = z.infer<typeof UploadBeginRequestSchema>;
export type UploadBeginResponse = z.infer<typeof UploadBeginResponseSchema>;
export type UploadChunkResponse = z.infer<typeof UploadChunkResponseSchema>;
// The demand message itself: the requester's held GET created a relay session
// and the holder streams bytes into it via `PUT /files/relay/:sessionId`.
export type FileRelayRequest = Extract<ServerMessage, { type: "file.requested" }>;
export type ClientMessage = z.infer<typeof ClientMessageSchema>;
export type ServerMessage = z.infer<typeof ServerMessageSchema>;
