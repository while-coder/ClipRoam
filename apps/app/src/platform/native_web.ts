import { BROWSER_CAPABILITIES } from "../utils/constants";
import type { PlatformCapabilities, ToastPayload } from "../types";
import type { ClipboardData, ClipboardWrite, SharedContent } from "./clipboard";
import type { FileStat, SaveTarget } from "./files";
import type { DeviceInfo, NativePlatform, PickedFiles, Unlisten } from "./native";

/** Browser operations use Web APIs without loading Tauri. */
export class NativeWeb implements NativePlatform {
  readonly kind = "web";
  readonly clipboardMonitoring = false;
  async getCapabilities(): Promise<PlatformCapabilities> { return {...BROWSER_CAPABILITIES}; }
  async getDeviceInfo(): Promise<DeviceInfo> { return {deviceName:"浏览器",cpu:"",osType:"browser",osVersion:navigator.userAgent,appVersion:"web"}; }
  private unavailable(): never { throw new Error("此操作需要桌面或手机客户端"); }
  async openAppDataDir(): Promise<void> { this.unavailable(); }
  async startWindowDrag(): Promise<void> { this.unavailable(); }
  async setQuickPasteMode(_enabled: boolean): Promise<boolean> { return this.unavailable(); }
  async capturePasteTarget(): Promise<void> { this.unavailable(); }
  async showToast(_payload: ToastPayload): Promise<void> { this.unavailable(); }
  async hideToast(): Promise<void> {}
  async listen<T>(_event: string, _handler: (payload: T) => void): Promise<Unlisten> { return () => {}; }
  async previewSource(path: string): Promise<string> { return path; }
  async readClipboard(_textOnly: boolean): Promise<ClipboardData | null> {
    const text = await navigator.clipboard.readText(); return text.trim() ? {kind:"text",text} : null;
  }
  async writeClipboard(payload: ClipboardWrite, _synthesize: boolean): Promise<void> {
    if (payload.kind === "text") {
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
  }
  async watchClipboard(_scope: string, _enabled: boolean): Promise<void> {}
  async pendingShares(_scope: string): Promise<SharedContent[]> { return []; }
  async acknowledgeShare(_id: string): Promise<void> {}
  async pickFiles(mode: string, _scope: string): Promise<PickedFiles> {
    const files = await new Promise<File[]>((resolve) => {
      const input = document.createElement("input"); input.type = "file"; input.multiple = true;
      if (mode === "folder") input.webkitdirectory = true;
      input.addEventListener("change", () => {resolve([...input.files ?? []]);input.remove();}, {once:true});
      input.addEventListener("cancel", () => {resolve([]);input.remove();}, {once:true});
      input.style.display = "none"; document.body.append(input); input.click();
    });
    return {files};
  }
  async getPaths(): Promise<{ appDir: string }> { return this.unavailable(); }
  async statFiles(_paths: string[]): Promise<(FileStat | null)[]> { return this.unavailable(); }
  async listFiles(_path: string): Promise<FileStat[]> { return this.unavailable(); }
  async readImage(_path: string): Promise<ClipboardData> { return this.unavailable(); }
  async readFile(_path: string, _offset: number, _length: number): Promise<Uint8Array> { return this.unavailable(); }
  async writeFile(_path: string, _bytes: Uint8Array, _append: boolean): Promise<void> { this.unavailable(); }
  async moveFile(_source: string, _destination: string): Promise<void> { this.unavailable(); }
  async copyFile(_source: string, _destination: string, _name?: string): Promise<string> { return this.unavailable(); }
  async createDirectory(_path: string, _name?: string): Promise<string> { return this.unavailable(); }
  async removeFile(_path: string): Promise<void> { this.unavailable(); }
  async selectSaveTarget(_single: boolean, fileName: string): Promise<SaveTarget> { return {kind:"browser",value:fileName}; }
}
