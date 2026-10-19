import { ClipboardKind } from "@cliproam/protocol";

export type PreviewFile = { name: string; path: string; fileId: string; size: number };

const textExtensions = new Set(["txt", "log", "md", "csv", "json", "xml", "yaml", "yml", "ini", "conf"]);
const imageExtensions = new Set(["png", "jpg", "jpeg", "webp", "bmp"]);

export type FilePreviewKind = typeof ClipboardKind.Text | typeof ClipboardKind.Image;

export function filePreviewKind(name: string): FilePreviewKind | undefined {
  const extension = name.split(".").pop()?.toLowerCase() ?? "";
  if (!name.includes(".")) return undefined;
  if (textExtensions.has(extension)) return ClipboardKind.Text;
  if (imageExtensions.has(extension)) return ClipboardKind.Image;
  return undefined;
}

/** Recognize Unicode BOMs; older Windows log files commonly use GBK. */
export function decodePreviewText(bytes: Uint8Array, truncated = false): string {
  // A preview may end inside a multi-byte character; leave that trailing character out.
  const options = { stream: truncated };
  if (bytes[0] === 0xff && bytes[1] === 0xfe) return new TextDecoder("utf-16le").decode(bytes, options);
  if (bytes[0] === 0xfe && bytes[1] === 0xff) return new TextDecoder("utf-16be").decode(bytes, options);
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes, options);
  } catch {
    return new TextDecoder("gb18030").decode(bytes, options);
  }
}
