import type { PlatformCapabilities, NotificationPayload } from "../types";
import type { ClipboardData, ClipboardWrite, SharedContent } from "./clipboard";
import type { FileStat, SaveTarget } from "./files";
import type { ShareReceiverEvent } from "../types";
import type { Ref } from "vue";

export type DeviceInfo = { deviceName: string; cpu: string; osType: string; osVersion: string; appVersion: string; architecture?: string };
export type PickedFiles = { paths?: string[]; files?: File[] };
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
  listenShares(handler: (event: ShareReceiverEvent) => void): Promise<Unlisten>;
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
  pendingShares(scope: string): Promise<SharedContent[]>;
  acknowledgeShare(id: string): Promise<void>;
  pickFiles(mode: string, scope: string): Promise<PickedFiles>;
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

export type { PlatformCapabilities, NotificationPayload, NotificationOptions, ShareReceiverEvent } from "../types";
export type { ClipboardData, ClipboardWrite, SharedContent } from "./clipboard";
export type { FileStat, SaveTarget } from "./files";
