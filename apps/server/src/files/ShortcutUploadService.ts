import { createHash } from "node:crypto";
import { createWriteStream } from "node:fs";
import { mkdtemp, open, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Transform } from "node:stream";
import { pipeline } from "node:stream/promises";
import type { FastifyRequest } from "fastify";
import { EntryPublishInputSchema, FILE_CHUNK_SIZE, FileInfoSchema, MAX_PUBLISH_BYTES, type EntryPublishInput, type FileInfo } from "@cliproam/protocol";
import { MEGABYTE, SMALL_JSON_BODY_LIMIT, type ServerConfig } from "../app/ServerConfig.js";
import { UploadHttpError, type UploadService } from "./UploadService.js";

type StagedFile = { path: string; name: string; fileId: string; size: number };

/** Stage and validate a complete request before publishing a record. */
export async function receiveShortcutUpload(
  request: FastifyRequest,
  userId: string,
  config: ServerConfig,
  uploads: Pick<UploadService, "begin" | "uploadPart">,
): Promise<Omit<EntryPublishInput, "sourceDeviceId">> {
  if (!request.isMultipart()) throw new UploadHttpError(415, "请使用 multipart/form-data 上传");
  const limit = config.maxStoredFileMb * MEGABYTE;
  if (limit <= 0) throw new UploadHttpError(413, "服务器未开启文件存储");
  const directory = await mkdtemp(join(tmpdir(), "cliproam-shortcut-"));
  const files: StagedFile[] = [];
  const tree: FileInfo = Object.create(null);
  let directoriesSeen = false;
  let nodes = 0;
  const addPath = (path: string, file?: StagedFile) => {
    const parts = path.replace(/\\/g, "/").split("/");
    if (parts.length > 64 || parts.some((part) => !part || part === "." || part === ".." || part === "__proto__" || part.length > 255 || /[:\x00-\x1f\x7f]/.test(part))) {
      throw new UploadHttpError(400, "文件名或相对路径无效");
    }
    let parent = tree;
    for (const [index, name] of parts.entries()) {
      const previous = parent[name];
      const leaf = file && index === parts.length - 1;
      if (leaf) {
        if (previous !== undefined) throw new UploadHttpError(400, "文件路径重复或与目录冲突");
        parent[name] = { f: file.fileId, s: file.size };
      } else {
        if (previous && typeof previous.f === "string") throw new UploadHttpError(400, "文件路径与目录冲突");
        if (!previous) parent[name] = Object.create(null);
        parent = parent[name] as FileInfo;
      }
      if (previous === undefined && ++nodes > 100_000) throw new UploadHttpError(413, "目录节点数超过服务器上限");
    }
  };
  try {
    for await (const part of request.parts({
      preservePath: true,
      limits: { fileSize: limit, files: config.maxCaptureFileCount,
        fields: 1, fieldSize: SMALL_JSON_BODY_LIMIT,
        parts: config.maxCaptureFileCount + 1 },
    })) {
      if (part.type === "field") {
        if (part.fieldname !== "directories" || directoriesSeen || part.valueTruncated || typeof part.value !== "string") {
          throw new UploadHttpError(400, "目录参数无效");
        }
        directoriesSeen = true;
        let directories: unknown;
        try { directories = JSON.parse(part.value); } catch { throw new UploadHttpError(400, "目录参数无效"); }
        if (!Array.isArray(directories) || directories.some((path) => typeof path !== "string")) throw new UploadHttpError(400, "目录参数无效");
        for (const path of directories) addPath(path);
        continue;
      }
      if (!["file", "files"].includes(part.fieldname) || !part.filename) throw new UploadHttpError(400, "请提供 file 或 files 文件字段");
      const hash = createHash("sha256");
      let size = 0;
      const path = join(directory, String(files.length));
      await pipeline(part.file, new Transform({
        transform(chunk: Buffer, _encoding, callback) {
          size += chunk.length;
          hash.update(chunk);
          callback(null, chunk);
        },
      }), createWriteStream(path, { flags: "wx" }));
      if (part.file.truncated || size >= limit) throw new UploadHttpError(413, "上传内容超过服务器存储上限");
      const file = { path, name: part.filename, fileId: hash.digest("hex"), size };
      files.push(file);
      addPath(file.name, file);
    }
    if (!Object.keys(tree).length) throw new UploadHttpError(400, "没有收到文件或目录");
    const parsed = FileInfoSchema.safeParse(tree);
    if (!parsed.success) throw new UploadHttpError(400, "文件目录结构无效");
    const entry: Omit<EntryPublishInput, "sourceDeviceId"> = {
      kind: "files", content: Object.keys(tree).join("、"), fileInfo: parsed.data,
    };
    if (Buffer.byteLength(JSON.stringify(entry)) > MAX_PUBLISH_BYTES) throw new UploadHttpError(413, "文件目录信息过大");
    if (!EntryPublishInputSchema.omit({ sourceDeviceId: true }).safeParse(entry).success) throw new UploadHttpError(400, "上传内容信息无效");
    // Reuse the normal upload limits, content hashes, chunk ledger and promotion.
    for (const file of files) {
      const begun = uploads.begin(userId, file.fileId, file.size, "files", file.name.split(/[\\/]/).at(-1));
      if (begun.status === "stored") continue;
      const missing = Buffer.from(begun.missingChunks, "base64");
      const handle = await open(file.path, "r");
      try {
        const buffer = Buffer.allocUnsafe(FILE_CHUNK_SIZE);
        for (let index = 0; index * FILE_CHUNK_SIZE < file.size; index++) {
          if (!(missing[index >> 3] & (1 << (index & 7)))) continue;
          const length = Math.min(FILE_CHUNK_SIZE, file.size - index * FILE_CHUNK_SIZE);
          let read = 0;
          while (read < length) {
            const result = await handle.read(buffer, read, length - read, index * FILE_CHUNK_SIZE + read);
            if (!result.bytesRead) throw new Error("上传暂存文件不完整");
            read += result.bytesRead;
          }
          uploads.uploadPart(file.fileId, index, buffer.subarray(0, length));
        }
      } finally { await handle.close(); }
    }
    return entry;
  } finally { await rm(directory, { recursive: true, force: true }); }
}
