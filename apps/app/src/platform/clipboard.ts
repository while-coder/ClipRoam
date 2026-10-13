import { bytesToBase64 } from "../utils/bytes";
import { isNative, nativeCall, nativeListen } from "./runtime";

export type ClipboardData =
  | { kind: "text"; text: string; html?: string; rtf?: string }
  | { kind: "files"; paths: string[] }
  | { kind: "image"; data: string; width: number; height: number; thumbnail: string };
export type ClipboardCapture = { id: string; signature: string; data: ClipboardData };
export type SharedContent = { id: string; text?: string; html?: string; images: string[]; paths: string[] };
export type ClipboardWrite = { kind: "text"; text: string; html?: string; rtf?: string }
  | { kind: "files"; paths: string[] } | { kind: "image"; bytes: Uint8Array };

/** OS permissions and browser APIs are contained here, away from capture/queue logic. */
export const clipboardPlatform = {
  monitoringAvailable: isNative,
  subscribe: (changed: () => void) => nativeListen("cliproam://clipboard-changed", changed),
  watch: (scope: string, enabled: boolean): Promise<void> => isNative ? nativeCall("native_watch_clipboard", {scope,enabled}) : Promise.resolve(),
  inbox: (scope: string): Promise<ClipboardCapture[]> => isNative ? nativeCall("native_clipboard_inbox", {scope}) : Promise.resolve([]),
  acknowledge: (scope: string, id: string): Promise<void> => isNative ? nativeCall("native_clipboard_ack", {scope,id}) : Promise.resolve(),
  shares: (scope: string): Promise<SharedContent[]> => isNative ? nativeCall("native_pending_shares", {scope}) : Promise.resolve([]),
  acknowledgeShare: (id: string): Promise<void> => isNative ? nativeCall("native_ack_share", {id}) : Promise.resolve(),
  async readText(): Promise<ClipboardData | null> {
    if (isNative) return nativeCall("native_read_clipboard", {textOnly:true});
    const text = await navigator.clipboard.readText(); return text.trim() ? {kind:"text",text} : null;
  },
  async pickFiles(mode: string, scope: string): Promise<{paths?: string[]; files?: File[]}> {
    if (isNative) return {paths:await nativeCall<string[]>("native_pick_files", {mode,scope})};
    const files = await new Promise<File[]>((resolve) => {
      const input = document.createElement("input"); input.type = "file"; input.multiple = true;
      if (mode === "folder") input.webkitdirectory = true;
      input.addEventListener("change", () => {resolve([...input.files ?? []]);input.remove();}, {once:true});
      input.addEventListener("cancel", () => {resolve([]);input.remove();}, {once:true});
      input.style.display = "none"; document.body.append(input); input.click();
    });
    return {files};
  },
  async write(payload: ClipboardWrite, synthesize: boolean): Promise<void> {
    if (isNative) {
      const nativePayload = payload.kind === "image" ? {kind:"image",data:bytesToBase64(payload.bytes)} : payload;
      await nativeCall("native_write_clipboard", {payload:nativePayload,synthesize});
    } else if (payload.kind === "text") {
      if (payload.html && typeof ClipboardItem !== "undefined") {
        await navigator.clipboard.write([new ClipboardItem({
          "text/plain":new Blob([payload.text],{type:"text/plain"}), "text/html":new Blob([payload.html],{type:"text/html"}),
        })]);
      } else await navigator.clipboard.writeText(payload.text);
    } else if (payload.kind === "image") {
      const image = await createImageBitmap(new Blob([payload.bytes]));
      const canvas = document.createElement("canvas"); canvas.width=image.width; canvas.height=image.height;
      canvas.getContext("2d")!.drawImage(image,0,0); image.close();
      const blob = await new Promise<Blob>((resolve,reject) => canvas.toBlob((blob) => blob ? resolve(blob) : reject(new Error("图片转换失败")),"image/png"));
      await navigator.clipboard.write([new ClipboardItem({"image/png":blob})]);
    } else throw new Error("浏览器不支持系统文件剪贴板，请下载文件");
  },
};
