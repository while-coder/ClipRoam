import { z } from "zod";

export const DEFAULT_AUTO_UPLOAD_LIMIT = 10 * 1024 * 1024;
export const FILE_CHUNK_SIZE = 128 * 1024;

export const ClipboardKindSchema = z.enum(["text", "files", "image"]);

export const ClipboardFileSchema = z.object({
  id: z.string(),
  name: z.string(),
  size: z.number().int().nonnegative(),
  mime: z.string().nullish().transform((value) => value ?? undefined),
  sha256: z.string().nullish().transform((value) => value ?? undefined),
  location: z.enum(["device", "server"]),
  available: z.boolean(),
});

export const ClipboardEntrySchema = z.object({
  id: z.string(),
  clientId: z.string().uuid().optional(),
  kind: ClipboardKindSchema,
  content: z.string(),
  html: z.string().optional(),
  rtf: z.string().optional(),
  files: z.array(ClipboardFileSchema).default([]),
  sourceDeviceId: z.string(),
  createdAt: z.string(),
  pinned: z.boolean().default(false),
});

// The connection-time manifest is intentionally small. Full entry metadata is
// fetched only for records missing from the local history.
export const ClipboardManifestEntrySchema = z.object({
  id: z.string(),
  clientId: z.string().uuid().optional(),
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
  z.object({
    type: z.literal("clipboard.delete"),
    entryId: z.string(),
  }),
  z.object({
    type: z.literal("clipboard.fetch"),
    requestId: z.string().uuid(),
    entryIds: z.array(z.string()).min(1).max(200),
  }),
  z.object({
    type: z.literal("file.upload.begin"),
    transferId: z.string().uuid(),
    entryId: z.string(),
    clientId: z.string().uuid(),
    fileFullPath: z.string().min(1).max(4096),
    fileModifiedAt: z.number().int().nonnegative(),
    file: ClipboardFileSchema,
  }),
  z.object({
    type: z.literal("file.download"),
    transferId: z.string().uuid(),
    entryId: z.string(),
    fileId: z.string(),
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
    fileId: z.string().uuid(),
    offset: z.number().int().nonnegative(),
  }),
  z.object({
    type: z.literal("file.uploaded"),
    transferId: z.string().uuid(),
    fileId: z.string(),
  }),
  z.object({
    type: z.literal("file.source.request"),
    transferId: z.string().uuid(),
    entryId: z.string(),
    fileId: z.string(),
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
export type ClipboardEntry = z.infer<typeof ClipboardEntrySchema>;
export type ClipboardManifestEntry = z.infer<typeof ClipboardManifestEntrySchema>;
export type Device = z.infer<typeof DeviceSchema>;
export type AuthCredentials = z.infer<typeof AuthCredentialsSchema>;
export type AuthResponse = z.infer<typeof AuthResponseSchema>;
export type ClientMessage = z.infer<typeof ClientMessageSchema>;
export type ServerMessage = z.infer<typeof ServerMessageSchema>;
