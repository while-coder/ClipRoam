import { createHmac, randomBytes, timingSafeEqual } from "node:crypto";

type FileAccess = { userId: string; name: string; expires: number; preview?: boolean };
const ACCESS_LIFETIME_MS = 60 * 60 * 1000;

export function videoContentType(name: string): string | undefined {
  switch (name.split(".").pop()?.toLowerCase()) {
    case "mp4": case "m4v": return "video/mp4";
    case "webm": return "video/webm";
    case "ogv": return "video/ogg";
    case "mov": return "video/quicktime";
    case "mkv": return "video/x-matroska";
    case "avi": return "video/x-msvideo";
    case "wmv": return "video/x-ms-wmv";
    case "flv": return "video/x-flv";
    case "mpg": case "mpeg": return "video/mpeg";
    case "3gp": return "video/3gpp";
    case "3g2": return "video/3gpp2";
    default: return undefined;
  }
}

/** Short-lived, file-scoped URLs; the account session token never enters the URL. */
export class FileAccessTokens {
  readonly #key = randomBytes(32);

  issue(userId: string, entryId: string, fileId: string, name: string, preview = false): string {
    const payload = Buffer.from(JSON.stringify({ userId, name, preview, expires: Date.now() + ACCESS_LIFETIME_MS })).toString("base64url");
    return `${payload}.${this.#sign(entryId, fileId, payload).toString("base64url")}`;
  }

  verify(token: string, entryId: string, fileId: string): FileAccess | undefined {
    if (token.length > 4096) return;
    const [payload, signature, extra] = token.split(".");
    if (!payload || !signature || extra !== undefined) return;
    const expected = this.#sign(entryId, fileId, payload);
    const actual = Buffer.from(signature, "base64url");
    if (actual.length !== expected.length || !timingSafeEqual(actual, expected)) return;
    try {
      const access = JSON.parse(Buffer.from(payload, "base64url").toString()) as FileAccess;
      if (typeof access.userId !== "string" || typeof access.name !== "string"
        || !Number.isSafeInteger(access.expires) || access.expires <= Date.now()) return;
      return access;
    } catch { return; }
  }

  #sign(entryId: string, fileId: string, payload: string): Buffer {
    return createHmac("sha256", this.#key).update(JSON.stringify([entryId, fileId, payload])).digest();
  }
}

/** A single HTTP byte range, with an exclusive end. Invalid/unsatisfiable ranges return null. */
export function parseFileRange(header: string, size: number): { start: number; end: number } | null {
  const match = /^bytes=(\d*)-(\d*)$/.exec(header);
  if (!match || (!match[1] && !match[2]) || size === 0) return null;
  const first = match[1] ? Number(match[1]) : undefined;
  const last = match[2] ? Number(match[2]) : undefined;
  if ((first !== undefined && !Number.isSafeInteger(first)) || (last !== undefined && !Number.isSafeInteger(last))) return null;
  if (first === undefined) return last! > 0 ? { start: Math.max(0, size - last!), end: size } : null;
  if (first >= size || (last !== undefined && last < first)) return null;
  return { start: first, end: last === undefined ? size : Math.min(size, last + 1) };
}
