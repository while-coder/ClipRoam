import { ClipboardKind } from "@cliproam/protocol";
import { languageForFile } from "../../components/preview/textLanguages";
import { isFileNode, type FileInfo, type TreeNode } from "@cliproam/protocol";

export type PreviewFile = { name: string; path: string; fileId: string; size: number };
export type RtfdDocument = { name: string; path: string; body: PreviewFile; files: PreviewFile[]; tree: FileInfo };

/** RTFD is a directory package; attachment paths stay relative to its root. */
export function rtfdDocument(name: string, path: string, node: TreeNode): RtfdDocument | undefined {
  if (!/\.rtfd$/i.test(name) || isFileNode(node)) return;
  const body = Object.entries(node).find(([key, value]) => key.toLowerCase() === "txt.rtf" && isFileNode(value));
  if (!body || !isFileNode(body[1])) return;
  const files: PreviewFile[] = [];
  const pending = [{ path: "", node }];
  while (pending.length) {
    const current = pending.pop()!;
    if (isFileNode(current.node)) continue;
    for (const [key, value] of Object.entries(current.node)) {
      const relative = current.path ? `${current.path}/${key}` : key;
      if (isFileNode(value)) files.push({ name: key, path: relative, fileId: value.f, size: value.s });
      else pending.push({ path: relative, node: value });
    }
  }
  return { name, path, body: { name: body[0], path: body[0], fileId: body[1].f, size: body[1].s }, files, tree: node };
}

const imageExtensions = new Set(["png", "jpg", "jpeg", "webp", "bmp"]);
const videoExtensions = new Set(["mp4", "m4v", "webm", "ogv", "mov", "mkv", "avi", "wmv", "flv", "mpg", "mpeg", "3gp", "3g2"]);
const audioTypes: Record<string, string> = {
  mp3: "audio/mpeg", wav: "audio/wav", ogg: "audio/ogg", oga: "audio/ogg",
  opus: "audio/ogg", m4a: "audio/mp4", aac: "audio/aac", flac: "audio/flac", weba: "audio/webm",
};

export const FilePreviewKind = {
  Text: ClipboardKind.Text,
  Image: ClipboardKind.Image,
  Audio: "audio",
  Video: "video",
  RichText: "rich-text",
} as const;
export type FilePreviewKind = typeof FilePreviewKind[keyof typeof FilePreviewKind];

export function audioMimeType(name: string): string | undefined {
  const extension = name.split(".").pop()?.toLowerCase() ?? "";
  return Object.prototype.hasOwnProperty.call(audioTypes, extension) ? audioTypes[extension] : undefined;
}

export function filePreviewKind(name: string): FilePreviewKind | undefined {
  const extension = name.split(".").pop()?.toLowerCase() ?? "";
  if (!name.includes(".")) return undefined;
  if (extension === "rtf" || extension === "rtfd") return FilePreviewKind.RichText;
  if (languageForFile(name)) return ClipboardKind.Text;
  if (imageExtensions.has(extension)) return ClipboardKind.Image;
  if (audioMimeType(name)) return FilePreviewKind.Audio;
  if (videoExtensions.has(extension)) return FilePreviewKind.Video;
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
