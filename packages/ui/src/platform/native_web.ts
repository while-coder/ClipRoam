import { BROWSER_CAPABILITIES } from "../config";
import type { PlatformCapabilities, ShareReceiverEvent, ToastPayload } from "../types";
import type { ClipboardData, ClipboardWrite, SharedContent } from "./clipboard";
import type { FileStat, SaveTarget } from "./files";
import type { DeviceInfo, DirectoryEntry, NativePlatform, PickedFiles, Unlisten, WindowModeOptions } from "./native";

/** Browser operations use Web APIs without loading Tauri. */
export class NativeWeb implements NativePlatform {
  readonly kind = "web";
  readonly clipboardMonitoring = false;
  readonly windowLabel = "main";
  async getAppVersion(): Promise<string> { return "web"; }
  async openExternalUrl(url: string): Promise<void> {
    const link = document.createElement("a");
    link.href = url;
    link.target = "_blank";
    link.rel = "noopener noreferrer";
    link.click();
  }
  async initializeWindowMode(): Promise<void> {}
  async showMainWindow(): Promise<void> {}
  async showPasteWindow(): Promise<void> {}
  async hideWindow(): Promise<void> {}
  async onFocusChanged(handler: (focused: boolean) => void): Promise<Unlisten> {
    const focus = () => handler(true), blur = () => handler(false);
    window.addEventListener("focus", focus); window.addEventListener("blur", blur);
    return () => { window.removeEventListener("focus", focus); window.removeEventListener("blur", blur); };
  }
  async listenShares(_handler: (event: ShareReceiverEvent) => void): Promise<Unlisten> { return () => {}; }
  async listenToast(_handler: (payload: ToastPayload) => void): Promise<Unlisten> { return () => {}; }
  async getCapabilities(): Promise<PlatformCapabilities> { return {...BROWSER_CAPABILITIES}; }
  async getDeviceInfo(): Promise<DeviceInfo> { return {deviceName:"浏览器",cpu:"",osType:"browser",osVersion:navigator.userAgent,appVersion:"web"}; }
  private unavailable(): never { throw new Error("此操作需要桌面或手机客户端"); }
  async openAppDataDir(): Promise<void> { this.unavailable(); }
  async startWindowDrag(): Promise<void> { this.unavailable(); }
  async setWindowMode(_options: WindowModeOptions): Promise<boolean> { return this.unavailable(); }
  async capturePasteTarget(): Promise<void> { this.unavailable(); }
  async showToast(_payload: ToastPayload): Promise<void> { this.unavailable(); }
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
  async selectSaveTarget(single: boolean, fileName: string): Promise<SaveTarget | null> {
    const picker = (window as Window & {
      showDirectoryPicker?: (options: {mode: "readwrite"}) => Promise<FileSystemDirectoryHandle>;
    }).showDirectoryPicker;
    if (!single && picker) {
      try {
        const directory = await picker.call(window, {mode:"readwrite"});
        return {kind:"browserDirectory",value:directory.name,directory};
      } catch (error) {
        if (error instanceof DOMException && error.name === "AbortError") return null;
        throw error;
      }
    }
    if (!single) throw new Error("当前浏览器不支持目录保存，无法导出文件夹或多文件");
    return {kind:"browser",value:fileName};
  }
  async exportDirectory(target: SaveTarget, entries: AsyncIterable<DirectoryEntry>, signal: AbortSignal): Promise<number> {
    if (target.kind !== "browserDirectory") throw new Error("未选择浏览器导出目录");
    let count = 0;
    for await (const entry of entries) {
      signal.throwIfAborted();
      if (!entry.path.length || entry.path.some((name) => !name || name === "." || name === ".." || /[/\\]/.test(name))) {
        throw new Error("导出文件路径不合法");
      }
      let directory = target.directory;
      const folders = entry.stream ? entry.path.slice(0, -1) : entry.path;
      for (const name of folders) {
        signal.throwIfAborted();
        directory = await directory.getDirectoryHandle(name, {create:true});
      }
      if (entry.stream) {
        const file = await directory.getFileHandle(entry.path[entry.path.length - 1]!, {create:true});
        const writable = await file.createWritable();
        await entry.stream.pipeTo(writable, {signal});
        count++;
      }
    }
    return count;
  }
}
