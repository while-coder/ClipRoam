import type { AccountPreferences, PlatformCapabilities } from "./types";

export const DEFAULT_SERVER_PROTOCOL = "http";
export const DEFAULT_SERVER_ADDRESS = "127.0.0.1:4810";
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
  shareReceiver: false,
};

export const BROWSER_CAPABILITIES: PlatformCapabilities = {
  mobile: false, clipboardMonitoring: false, globalShortcut: false, automaticPaste: false,
  fileClipboard: false, imageClipboard: true, nativeFileExport: true, openDataDirectory: false, shareReceiver: false,
};

// 三端共用的账号偏好默认值，由各客户端的 native 能力决定是否启用。
export const DEFAULT_AUTO_UPLOAD_LIMIT_MB = 50;
export const DEFAULT_AUTO_UPLOAD_LIMIT = DEFAULT_AUTO_UPLOAD_LIMIT_MB * 1024 * 1024;
export const DEFAULT_AUTO_RECEIVE_CLIPBOARD = true;
// 是否监听本机剪贴板（与 Rust 侧 default_watch_clipboard 一致）。
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
    autoReceiveClipboard: DEFAULT_AUTO_RECEIVE_CLIPBOARD,
    watchClipboard: DEFAULT_WATCH_CLIPBOARD,
    excludePatterns: [...DEFAULT_EXCLUDE_PATTERNS],
    serverMaxFileMb: DEFAULT_SERVER_MAX_FILE_MB,
    serverMaxImageMb: 10,
    serverMaxHistoryEntries: 3000,
    maxCaptureFileCount: DEFAULT_MAX_CAPTURE_FILE_COUNT,
  };
}
