import type { EntrySummary, PlatformCapabilities } from "../types";

export const CONFIGURED_SERVER_ADDRESS = "127.0.0.1:4810";
export const CONFIGURED_SERVER_PROTOCOL = "http";
export const DEFAULT_SERVER_ADDRESS = CONFIGURED_SERVER_ADDRESS.includes("://")
  ? new URL(CONFIGURED_SERVER_ADDRESS).host
  : CONFIGURED_SERVER_ADDRESS;
export const BROWSER_CONFIG_KEY = "cliproam.syncConfig";

export const PAGE_SIZE = 50;

export const EMPTY_SUMMARY: EntrySummary = {
  rootKind: "",
  fileCount: 0,
  hashedCount: 0,
  contentCount: 0,
  totalSize: 0,
  maxFileSize: 0,
  uploadedCount: 0,
  readyCount: 0,
  pendingCount: 0,
  pendingSize: 0,
};

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
