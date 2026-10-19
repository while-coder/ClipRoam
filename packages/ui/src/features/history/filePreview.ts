import { ClipboardKind } from "@cliproam/protocol";
import { languageForFile } from "../../components/preview/textLanguages";

export type PreviewFile = { name: string; path: string; fileId: string; size: number };

const imageExtensions = new Set(["png", "jpg", "jpeg", "webp", "bmp"]);
const audioTypes: Record<string, string> = {
  mp3: "audio/mpeg", wav: "audio/wav", ogg: "audio/ogg", oga: "audio/ogg",
  opus: "audio/ogg", m4a: "audio/mp4", aac: "audio/aac", flac: "audio/flac", weba: "audio/webm",
};

export const FilePreviewKind = {
  Text: ClipboardKind.Text,
  Image: ClipboardKind.Image,
  Audio: "audio",
} as const;
export type FilePreviewKind = typeof FilePreviewKind[keyof typeof FilePreviewKind];

export function audioMimeType(name: string): string | undefined {
  const extension = name.split(".").pop()?.toLowerCase() ?? "";
  return Object.prototype.hasOwnProperty.call(audioTypes, extension) ? audioTypes[extension] : undefined;
}

export function filePreviewKind(name: string): FilePreviewKind | undefined {
  const extension = name.split(".").pop()?.toLowerCase() ?? "";
  if (!name.includes(".")) return undefined;
  if (languageForFile(name)) return ClipboardKind.Text;
  if (imageExtensions.has(extension)) return ClipboardKind.Image;
  if (audioMimeType(name)) return FilePreviewKind.Audio;
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
