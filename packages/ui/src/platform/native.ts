import type { PlatformCapabilities, NotificationPayload } from "../types";
import type { ClipboardData, ClipboardWrite } from "./clipboard";
import type { FileStat, SaveTarget } from "./files";
import type { Ref } from "vue";

export type DeviceInfo = {
  /** 用户设置的别名；为空时显示系统设备名。 */
  deviceAlias?: string;
  /** 系统设备名。 */
  deviceName: string;
  /** Hardware model reported by the OS; omitted when unavailable. */
  deviceModel?: string;
  /** Consumer brand reported by the mobile OS. */
  deviceBrand?: string;
  /** Marketing model name from vendor properties, when available. */
  deviceMarketName?: string;
  androidApiLevel?: number;
  /** Installed Android versionCode / iOS CFBundleVersion. */
  appBuildNumber?: string;
  /** Total physical memory in bytes; omitted when unavailable. */
  memoryTotalBytes?: number;
  cpu: string;
  /** Native OS type, or browser name for Web/PWA/extensions. */
  osType: string;
  /** Native OS version, or the browser's reported version (possibly major-only). */
  osVersion: string;
  appVersion: string;
  architecture?: string;
  /** Operating system username; unavailable in browsers. */
  username?: string;
  /** Browser/WebView preferred language, distinct from the OS locale. */
  language: string;
  timeZone?: string;
  systemLocale?: string;
};
export type PickedFiles = { paths?: string[]; files?: File[] };
export const FilePickerMode = { File: "file", Folder: "folder" } as const;
export type FilePickerMode = typeof FilePickerMode[keyof typeof FilePickerMode];
export type Unlisten = () => void;
export type DirectoryEntry = { path: string[]; stream?: ReadableStream<Uint8Array> };

export type QuickPasteShortcutState = {
  shortcut: Ref<string>;
  status: Ref<{state: "idle" | "ok" | "error"; message: string}>;
  refreshing: Ref<boolean>;
  display(shortcut: string): string;
  initialize(): Promise<boolean>;
  save(): Promise<boolean>;
  resetDraft(): void;
  dispose(): Promise<void>;
};

/** Platform operations independent of Rust command names and wire formats. */
export interface NativePlatform {
  readonly kind: "rust" | "web";
  readonly clipboardMonitoring: boolean;
  readonly windowLabel: string;
  readonly quickPasteShortcut?: QuickPasteShortcutState;
  getAppVersion(): Promise<string>;
  openExternalUrl(url: string): Promise<void>;
  showMainWindow(options?: { focusSearch?: boolean }): Promise<void>;
  showPasteWindow(): Promise<void>;
  hideWindow(): Promise<void>;
  onFocusChanged(handler: (focused: boolean) => void): Promise<Unlisten>;
  /** Native file drops; web clients use DOM drag events. */
  onFileDrop?(handler: (files: PickedFiles) => void): Promise<Unlisten>;
  getCapabilities(): Promise<PlatformCapabilities>;
  getDeviceInfo(): Promise<DeviceInfo>;
  openAppDataDir(): Promise<void>;
  startWindowDrag(): Promise<void>;
  showNotification(payload: NotificationPayload): Promise<void>;
  listen<T>(event: string, handler: (payload: T) => void): Promise<Unlisten>;
  previewSource(path: string): Promise<string>;
  readClipboard(textOnly: boolean): Promise<ClipboardData | null>;
  writeClipboard(payload: ClipboardWrite, synthesize: boolean): Promise<void>;
  watchClipboard(scope: string, enabled: boolean): Promise<void>;
  pickFiles(mode: FilePickerMode, scope: string): Promise<PickedFiles>;
  getPaths(): Promise<{ appDir: string }>;
  statFiles(paths: string[]): Promise<(FileStat | null)[]>;
  listFiles(path: string): Promise<FileStat[]>;
  readImage(path: string): Promise<ClipboardData>;
  readFile(path: string, offset: number, length: number): Promise<Uint8Array>;
  writeFile(path: string, bytes: Uint8Array, append: boolean): Promise<void>;
  moveFile(source: string, destination: string): Promise<void>;
  copyFile(source: string, destination: string, name?: string): Promise<string>;
  createDirectory(path: string, name?: string): Promise<string>;
  removeFile(path: string): Promise<void>;
  selectSaveTarget(single: boolean, fileName: string): Promise<SaveTarget | null>;
  exportDirectory(target: SaveTarget, entries: AsyncIterable<DirectoryEntry>, signal: AbortSignal): Promise<number>;
}

export let native: NativePlatform;

export function configureNative(platform: NativePlatform): void {
  native = platform;
}

export type { PlatformCapabilities, NotificationPayload, NotificationOptions } from "../types";
export type { ClipboardData, ClipboardWrite } from "./clipboard";
export type { FileStat, SaveTarget } from "./files";
