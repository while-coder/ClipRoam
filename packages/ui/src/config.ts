import type { AccountPreferences, PlatformCapabilities } from "./types";

export const DEFAULT_SERVER_ADDRESS = "http://127.0.0.1:4810";
export const PAGE_SIZE = 50;

export const DESKTOP_CAPABILITIES: PlatformCapabilities = {
  mobile: false,
  clipboardMonitoring: true,
  globalShortcut: true,
  automaticPaste: true,
  fileClipboard: true,
  imageClipboard: true,
  nativeFileExport: true,
  openDataDirectory: true,
};

export const BROWSER_CAPABILITIES: PlatformCapabilities = {
  mobile: false, clipboardMonitoring: false, globalShortcut: false, automaticPaste: false,
  fileClipboard: false, imageClipboard: true, nativeFileExport: true, openDataDirectory: false,
};

// 三端共用的账号偏好默认值，由各客户端的 native 能力决定是否启用。
export const DEFAULT_AUTO_UPLOAD_LIMIT_MB = 50;
export const DEFAULT_AUTO_UPLOAD_LIMIT = DEFAULT_AUTO_UPLOAD_LIMIT_MB * 1024 * 1024;
export const DEFAULT_AUTO_RECEIVE_CLIPBOARD = true;
// 是否默认监听本机剪贴板的各类内容。
export const DEFAULT_WATCH_CLIPBOARD = true;
// 捕获文件/文件夹时按名称跳过的默认过滤模式（与 Rust 侧 default_exclude_patterns 一致）。
export const DEFAULT_EXCLUDE_PATTERNS = ["node_modules"];
// 服务器单文件存储上限的兜底显示值；服务器下发的 settings.maxStoredFileMb 才是权威值。
export const DEFAULT_SERVER_MAX_FILE_MB = 200;
// 单次复制文件数上限的兜底值；服务器下发的 settings.maxCaptureFileCount 才是权威值。
const DEFAULT_MAX_CAPTURE_FILE_COUNT = 1000;

export function defaultAccountPreferences(): AccountPreferences {
  return {
    autoUploadLimitMb: DEFAULT_AUTO_UPLOAD_LIMIT_MB,
    autoReceiveClipboardText: DEFAULT_AUTO_RECEIVE_CLIPBOARD,
    autoReceiveClipboardImage: DEFAULT_AUTO_RECEIVE_CLIPBOARD,
    watchClipboardText: DEFAULT_WATCH_CLIPBOARD,
    watchClipboardImage: DEFAULT_WATCH_CLIPBOARD,
    watchClipboardFiles: DEFAULT_WATCH_CLIPBOARD,
    excludePatterns: [...DEFAULT_EXCLUDE_PATTERNS],
    serverMaxFileMb: DEFAULT_SERVER_MAX_FILE_MB,
    serverMaxImageMb: 10,
    serverMaxHistoryEntries: 3000,
    maxCaptureFileCount: DEFAULT_MAX_CAPTURE_FILE_COUNT,
  };
}

/** Each new option inherits its former master switch until explicitly saved. */
export function normalizeAccountPreferences(stored?: Partial<AccountPreferences> & {
  autoReceiveClipboard?: boolean; watchClipboard?: boolean;
}): AccountPreferences {
  const { autoReceiveClipboard, watchClipboard, ...preferences } = stored ?? {};
  const defaults = defaultAccountPreferences();
  return {
    ...defaults,
    ...preferences,
    autoReceiveClipboardText: preferences.autoReceiveClipboardText ?? autoReceiveClipboard ?? defaults.autoReceiveClipboardText,
    autoReceiveClipboardImage: preferences.autoReceiveClipboardImage ?? autoReceiveClipboard ?? defaults.autoReceiveClipboardImage,
    watchClipboardText: preferences.watchClipboardText ?? watchClipboard ?? defaults.watchClipboardText,
    watchClipboardImage: preferences.watchClipboardImage ?? watchClipboard ?? defaults.watchClipboardImage,
    watchClipboardFiles: preferences.watchClipboardFiles ?? watchClipboard ?? defaults.watchClipboardFiles,
  };
}
