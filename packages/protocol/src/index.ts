import { z } from "zod";

export const DEFAULT_AUTO_UPLOAD_LIMIT = 10 * 1024 * 1024;
export const FILE_CHUNK_SIZE = 128 * 1024;
// A single entry carries its whole directory tree, so publish/fetch messages can
// get large. The tree is compact (~40 bytes per node) but unbounded by design.
export const MAX_MESSAGE_BYTES = 16 * 1024 * 1024;
export const ENTRY_FETCH_BATCH = 20;

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

export const AuthCredentialsSchema = z.object({
  username: z.string().trim().min(3).max(32).regex(/^[a-zA-Z0-9_.-]+$/),
  password: z.string().min(6).max(128),
  deviceId: z.string().min(1).max(100),
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

export const ClientMessageSchema = z.discriminatedUnion("type", [
  z.object({
    type: z.literal("auth"),
    token: z.string().min(1),
    device: DeviceSchema,
  }),
  z.object({
    type: z.literal("clipboard.publish"),
    entry: ClipboardEntrySchema,
  }),
  // A durable history update and a live clipboard change are different
  // events. Reconnect restores and metadata edits publish entries without
  // unexpectedly replacing the clipboard on every other device.
  z.object({
    type: z.literal("clipboard.activate"),
    entryId: z.string(),
  }),
  z.object({
    type: z.literal("clipboard.delete"),
    entryId: z.string(),
  }),
  z.object({
    type: z.literal("clipboard.fetch"),
    requestId: z.string().uuid(),
    entryIds: z.array(z.string()).min(1).max(ENTRY_FETCH_BATCH),
  }),
  // Uploads are addressed purely by content, so the server needs no entry
  // context: it either already holds these bytes (instant) or it does not.
  z.object({
    type: z.literal("file.upload.begin"),
    transferId: z.string().uuid(),
    fileId: FileIdSchema,
    size: z.number().int().nonnegative(),
  }),
  z.object({
    type: z.literal("file.download"),
    transferId: z.string().uuid(),
    entryId: z.string(),
    fileId: FileIdSchema,
    sourceDeviceId: z.string(),
  }),
  z.object({
    type: z.literal("file.chunk"),
    transferId: z.string().uuid(),
    data: z.string(),
  }),
  z.object({
    type: z.literal("file.complete"),
    transferId: z.string().uuid(),
  }),
  z.object({
    type: z.literal("file.abort"),
    transferId: z.string().uuid(),
    message: z.string(),
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
    type: z.literal("clipboard.entries"),
    requestId: z.string().uuid(),
    entries: z.array(ClipboardEntrySchema),
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
  z.object({
    type: z.literal("file.upload.ready"),
    transferId: z.string().uuid(),
    offset: z.number().int().nonnegative(),
  }),
  // Also sent instead of `file.upload.ready` when the server already holds the
  // content, which lets the client skip the transfer entirely.
  z.object({
    type: z.literal("file.uploaded"),
    transferId: z.string().uuid(),
    fileId: FileIdSchema,
  }),
  // A file may become available after its entry was already synced to another
  // device. Keep that per-file state current without re-sending the entry.
  z.object({
    type: z.literal("file.available"),
    fileId: FileIdSchema,
  }),
  z.object({
    type: z.literal("file.source.request"),
    transferId: z.string().uuid(),
    entryId: z.string(),
    fileId: FileIdSchema,
  }),
  z.object({
    type: z.literal("file.chunk"),
    transferId: z.string().uuid(),
    data: z.string(),
  }),
  z.object({
    type: z.literal("file.complete"),
    transferId: z.string().uuid(),
  }),
  z.object({
    type: z.literal("file.failed"),
    transferId: z.string().uuid(),
    message: z.string(),
  }),
  z.object({ type: z.literal("pong") }),
  z.object({
    type: z.literal("error"),
    code: z.string(),
    message: z.string(),
  }),
]);

export type ClipboardKind = z.infer<typeof ClipboardKindSchema>;
export type ClipboardFile = z.infer<typeof ClipboardFileSchema>;
export type ClipboardTree = z.infer<typeof ClipboardTreeSchema>;
export type ClipboardTreeRoot = ClipboardTree["roots"][number];
export type ClipboardEntry = z.infer<typeof ClipboardEntrySchema>;
export type ClipboardManifestEntry = z.infer<typeof ClipboardManifestEntrySchema>;
export type Device = z.infer<typeof DeviceSchema>;
export type AuthCredentials = z.infer<typeof AuthCredentialsSchema>;
export type AuthResponse = z.infer<typeof AuthResponseSchema>;
export type ClientMessage = z.infer<typeof ClientMessageSchema>;
export type ServerMessage = z.infer<typeof ServerMessageSchema>;
