import type { PlatformCapabilities, ToastPayload, ClipboardData, ClipboardWrite, SharedContent,
  FileStat, SaveTarget, DeviceInfo, DirectoryEntry, NativePlatform, PickedFiles, Unlisten, ShareReceiverEvent } from "@cliproam/ui/native";
import { base64ToBytes, bytesToBase64 } from "@cliproam/ui/bytes";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import { addPluginListener } from "@tauri-apps/api/core";
import { useTauriUpdater } from "@while-coder/tauri-updater-vue";

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
type Listen = <T>(event: string, handler: (payload: T) => void) => Promise<Unlisten>;
const invokeRust: Invoke = async (command, args) => (await import("@tauri-apps/api/core")).invoke(command, args);
async function listenRust<T>(event: string, handler: (payload: T) => void): Promise<Unlisten> {
  return (await import("@tauri-apps/api/event")).listen<T>(event, ({payload}) => handler(payload));
}

/** Custom Rust commands and their serialized payloads are contained here. */
export class NativeRust implements NativePlatform {
  readonly kind = "rust";
  readonly clipboardMonitoring = true;
  readonly updater = useTauriUpdater();
  get windowLabel(): string { return getCurrentWindow().label; }
  getAppVersion(): Promise<string> { return getVersion(); }
  async initializeWindowMode(): Promise<void> { return (await import("../features/quick-paste/pasteWindow")).initializeWindowMode(); }
  async showMainWindow(): Promise<void> { return (await import("../features/quick-paste/pasteWindow")).showMainWindow(); }
  async showPasteWindow(): Promise<void> { return (await import("../features/quick-paste/pasteWindow")).showPasteWindow(); }
  async hideWindow(): Promise<void> { return (await import("../features/quick-paste/pasteWindow")).hideWindow(); }
  onFocusChanged(handler: (focused: boolean) => void): Promise<Unlisten> {
    return getCurrentWindow().onFocusChanged(({payload}) => handler(payload));
  }
  async listenShares(handler: (event: ShareReceiverEvent) => void): Promise<Unlisten> {
    const listener = await addPluginListener("cliproam-share-receiver", "received", handler);
    return () => { void listener.unregister(); };
  }
  listenToast(handler: (payload: ToastPayload) => void): Promise<Unlisten> {
    return this.subscribe("cliproam://toast", handler);
  }
  async registerQuickPasteShortcut(shortcut: string): Promise<void> {
    const { register } = await import("@tauri-apps/plugin-global-shortcut");
    await register(shortcut, (event) => {
      if (event.state === "Pressed") void import("@tauri-apps/api/event").then(({emitTo}) => emitTo("main", "cliproam://show-paste"));
    });
  }
  async isShortcutRegistered(shortcut: string): Promise<boolean> {
    return (await import("@tauri-apps/plugin-global-shortcut")).isRegistered(shortcut);
  }
  async unregisterShortcut(shortcut: string): Promise<void> {
    return (await import("@tauri-apps/plugin-global-shortcut")).unregister(shortcut);
  }
  constructor(private readonly invoke: Invoke = invokeRust, private readonly subscribe: Listen = listenRust) {}
  getCapabilities(): Promise<PlatformCapabilities> { return this.invoke("native_get_platform_capabilities"); }
  getDeviceInfo(): Promise<DeviceInfo> { return this.invoke("native_get_device_info"); }
  openAppDataDir(): Promise<void> { return this.invoke("native_open_app_data_dir"); }
  startWindowDrag(): Promise<void> { return this.invoke("native_start_window_drag"); }
  setQuickPasteMode(enabled: boolean): Promise<boolean> { return this.invoke("native_set_quick_paste_mode", {enabled}); }
  capturePasteTarget(): Promise<void> { return this.invoke("native_capture_paste_target"); }
  showToast(payload: ToastPayload): Promise<void> { return this.invoke("native_show_toast", {...payload}); }
  hideToast(): Promise<void> { return this.invoke("native_hide_toast"); }
  listen<T>(event: string, handler: (payload: T) => void): Promise<Unlisten> { return this.subscribe(event, handler); }
  async previewSource(path: string): Promise<string> {
    if (/^(blob:|data:|https?:)/.test(path)) return path;
    return (await import("@tauri-apps/api/core")).convertFileSrc(path);
  }
  readClipboard(textOnly: boolean): Promise<ClipboardData | null> { return this.invoke("native_read_clipboard", {textOnly}); }
  writeClipboard(payload: ClipboardWrite, synthesize: boolean): Promise<void> {
    const encoded = payload.kind === "image" ? {kind:"image",data:bytesToBase64(payload.bytes)} : payload;
    return this.invoke("native_write_clipboard", {payload:encoded,synthesize});
  }
  watchClipboard(scope: string, enabled: boolean): Promise<void> { return this.invoke("native_watch_clipboard", {scope,enabled}); }
  pendingShares(scope: string): Promise<SharedContent[]> { return this.invoke("native_pending_shares", {scope}); }
  acknowledgeShare(id: string): Promise<void> { return this.invoke("native_ack_share", {id}); }
  async pickFiles(mode: string, scope: string): Promise<PickedFiles> { return {paths:await this.invoke<string[]>("native_pick_files", {mode,scope})}; }
  getPaths(): Promise<{ appDir: string }> { return this.invoke("native_paths"); }
  statFiles(paths: string[]): Promise<(FileStat | null)[]> { return this.invoke("native_fs_stat", {paths}); }
  listFiles(path: string): Promise<FileStat[]> { return this.invoke("native_fs_list", {path}); }
  readImage(path: string): Promise<ClipboardData> { return this.invoke("native_read_image", {path}); }
  async readFile(path: string, offset: number, length: number): Promise<Uint8Array> { return base64ToBytes(await this.invoke<string>("native_fs_read", {path,offset,length})); }
  writeFile(path: string, bytes: Uint8Array, append: boolean): Promise<void> { return this.invoke("native_fs_write", {path,data:bytesToBase64(bytes),append}); }
  moveFile(source: string, destination: string): Promise<void> { return this.invoke("native_fs_move", {source,destination}); }
  copyFile(source: string, destination: string, name?: string): Promise<string> { return this.invoke("native_fs_copy", name === undefined ? {source,destination} : {source,destination,name}); }
  createDirectory(path: string, name?: string): Promise<string> { return this.invoke("native_fs_mkdir", name === undefined ? {path} : {path,name}); }
  removeFile(path: string): Promise<void> { return this.invoke("native_fs_remove", {path}); }
  selectSaveTarget(single: boolean, fileName: string): Promise<SaveTarget | null> { return this.invoke("native_save_dialog", {single,fileName}); }
  async exportDirectory(_target: SaveTarget, _entries: AsyncIterable<DirectoryEntry>, _signal: AbortSignal): Promise<number> {
    throw new Error("浏览器目录导出仅在 Web 平台可用");
  }
}
