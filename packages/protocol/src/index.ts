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
export const ENTRY_PAGE_MAX_LIMIT = 200;

export const ClipboardKindSchema = z.enum(["text", "files", "image"]);

// A file is addressed by the sha256 of its content, so identical bytes are one
// entity no matter how many entries, paths or devices reference them.
export const FileIdSchema = z.string().regex(/^[0-9a-f]{64}$/);

// Content-pool entry: describes bytes only. Names and paths live in the tree.
export const ClipboardFileSchema = z.object({
  fileId: FileIdSchema,
  size: z.number().int().nonnegative(),
  available: z.boolean().default(false),
});

// Structure of a `files` entry, kept compact: `p` is a relative path rooted at
// one of `roots`, `f` is the content it points at. Directories never occupy a
// content row, and duplicated content is a repeated `f`, not a repeated blob.
export const ClipboardTreeSchema = z.object({
  v: z.union([z.literal(1), z.literal(2)]),
  roots: z.array(z.object({
    name: z.string().min(1).max(255),
    kind: z.enum(["file", "dir"]),
  })),
  dirs: z.array(z.string().min(1).max(1024)).default([]),
  files: z.array(z.object({
    p: z.string().min(1).max(1024),
    f: FileIdSchema,
    // Original size remains available even when `b` points at a transfer pack.
    s: z.number().int().nonnegative().optional(),
    // Version 2 entries may transfer this original content inside a bounded
    // pack. `f` remains the identity used to restore and verify the file.
    b: FileIdSchema.optional(),
  })).default([]),
});

export const ClipboardEntrySchema = z.object({
  id: z.string(),
  kind: ClipboardKindSchema,
  content: z.string(),
  html: z.string().optional(),
  rtf: z.string().optional(),
  // A small WebP data payload for image-list rendering. Full-resolution image
  // bytes remain in the content pool and are only fetched for preview/paste.
  thumbnail: z.string().max(96 * 1024).optional(),
  tree: ClipboardTreeSchema.nullish().transform((value) => value ?? undefined),
  files: z.array(ClipboardFileSchema).default([]),
  sourceDeviceId: z.string(),
  createdAt: z.string(),
  pinned: z.boolean().default(false),
});

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
export const EntryPublishRequestSchema = z.object({
  deviceId: DeviceIdSchema,
  // The keyset cursor orders on `createdAt`, so it must be a fixed-width
  // ISO-8601 UTC string: a client-supplied non-ISO value would break
  // pagination even though it would otherwise store fine.
  entry: ClipboardEntrySchema.refine(
    (value) => /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{3})?Z$/.test(value.createdAt),
    { message: "createdAt 必须是 ISO-8601 UTC 时间" },
  ),
});

export const EntryPublishResponseSchema = z.object({ entry: ClipboardEntrySchema });

export const EntryQueryRequestSchema = z.object({
  entryIds: z.array(z.string()).min(1).max(ENTRY_QUERY_BATCH),
});

// Ids the server does not know are simply absent, not an error.
export const EntryQueryResponseSchema = z.object({ entries: z.array(ClipboardEntrySchema) });

// Keyset pagination: `nextCursor` feeds straight back into the next request,
// and null means the last page was reached.
export const EntryListResponseSchema = z.object({
  entries: z.array(ClipboardEntrySchema),
  nextCursor: z.string().nullable(),
});

export const EntryActivateRequestSchema = z.object({ deviceId: DeviceIdSchema });

export const EntryActivateResponseSchema = z.object({ entry: ClipboardEntrySchema });

// Opaque to clients: the server encodes the last entry of a page and expects
// the same token back on the next request.
export type EntryCursor = { createdAt: string; id: string };

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
  z.object({
    type: z.literal("auth.ack"),
    manifest: z.array(ClipboardManifestEntrySchema),
    devices: z.array(DeviceSchema),
  }),
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
// ledger. `missing` is a base64 bitmap over chunk indices where bit 1 means
// that chunk still has to be sent.
export const UploadBeginRequestSchema = z.object({
  fileId: FileIdSchema,
  size: z.number().int().nonnegative(),
});

export const UploadBeginResponseSchema = z.discriminatedUnion("status", [
  z.object({ status: z.literal(UploadStatus.stored), fileId: FileIdSchema }),
  z.object({
    status: z.literal(UploadStatus.ready),
    missing: z.string(),
    receivedBytes: z.number().int().nonnegative(),
  }),
]);

export const UploadChunkResponseSchema = z.discriminatedUnion("status", [
  z.object({ status: z.literal(UploadStatus.stored), fileId: FileIdSchema }),
  z.object({
    status: z.literal(UploadStatus.accepted),
    missing: z.string(),
    receivedBytes: z.number().int().nonnegative(),
  }),
]);

// Body of a `GET /files/:entryId/:fileId` 404 reply when the content is not
// on the server (yet). Any other 404 is a real refusal reported verbatim; a
// client that receives this code simply retries while the demand is served.
export const DOWNLOAD_NOT_STORED_CODE = "NOT_STORED";

// Body of a `GET /files/requests` long-poll: the account's outstanding download
// demands. A device that holds the listed content pushes it up through the
// upload routes; demands nobody serves simply expire on the server.
export const FileRequestsResponseSchema = z.object({
  requests: z.array(z.object({
    fileId: FileIdSchema,
    entryId: z.string(),
    size: z.number().int().nonnegative(),
  })),
});

export type ClipboardKind = z.infer<typeof ClipboardKindSchema>;
export type ClipboardFile = z.infer<typeof ClipboardFileSchema>;
export type ClipboardTree = z.infer<typeof ClipboardTreeSchema>;
export type ClipboardTreeRoot = ClipboardTree["roots"][number];
export type ClipboardEntry = z.infer<typeof ClipboardEntrySchema>;
export type ClipboardManifestEntry = z.infer<typeof ClipboardManifestEntrySchema>;
export type Device = z.infer<typeof DeviceSchema>;
export type AuthCredentials = z.infer<typeof AuthCredentialsSchema>;
export type AuthResponse = z.infer<typeof AuthResponseSchema>;
export type EntryPublishRequest = z.infer<typeof EntryPublishRequestSchema>;
export type EntryPublishResponse = z.infer<typeof EntryPublishResponseSchema>;
export type EntryQueryRequest = z.infer<typeof EntryQueryRequestSchema>;
export type EntryQueryResponse = z.infer<typeof EntryQueryResponseSchema>;
export type EntryListResponse = z.infer<typeof EntryListResponseSchema>;
export type EntryActivateResponse = z.infer<typeof EntryActivateResponseSchema>;
export type EntryActivateRequest = z.infer<typeof EntryActivateRequestSchema>;
export type UploadBeginRequest = z.infer<typeof UploadBeginRequestSchema>;
export type UploadBeginResponse = z.infer<typeof UploadBeginResponseSchema>;
export type UploadChunkResponse = z.infer<typeof UploadChunkResponseSchema>;
export type FileRequest = z.infer<typeof FileRequestsResponseSchema>["requests"][number];
export type FileRequestsResponse = z.infer<typeof FileRequestsResponseSchema>;
export type ClientMessage = z.infer<typeof ClientMessageSchema>;
export type ServerMessage = z.infer<typeof ServerMessageSchema>;
