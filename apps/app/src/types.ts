import type {
  ClipboardEntry,
  ClipboardKind,
  Device,
} from "@cliproam/protocol";

export type { ClipboardEntry, ClipboardKind, Device };

export type SyncConfig = {
  enabled: boolean;
  serverAddress: string;
  serverProtocol: "http" | "https";
  username: string;
  sessionToken: string;
  autoUploadLimitMb: number;
  autoReceiveClipboard: boolean;
};

export type MissingFile = { fileId: string; size: number; sourceDeviceId: string };
export type SavePreparation = { saveId: string; missing: MissingFile[] };
export type VirtualFileRequest = {
  entryId: string;
  fileId: string;
  size: number;
  sourceDeviceId: string;
};

export type PlatformCapabilities = {
  mobile: boolean;
  clipboardMonitoring: boolean;
  globalShortcut: boolean;
  automaticPaste: boolean;
  fileClipboard: boolean;
  imageClipboard: boolean;
  nativeFileExport: boolean;
  openDataDirectory: boolean;
  shareReceiver: boolean;
};

/**
 * Aggregates computed by the backend. A folder can hold thousands of nodes, so
 * the list never receives the tree itself — only these counters.
 */
export type EntrySummary = {
  rootKind: string;
  fileCount: number;
  hashedCount: number;
  contentCount: number;
  totalSize: number;
  maxFileSize: number;
  uploadedCount: number;
  readyCount: number;
  pendingCount: number;
  pendingSize: number;
  uploadableSize?: number;
  previewPath?: string;
};

export type LocalClipboardEntry = ClipboardEntry & { summary: EntrySummary };

/**
 * Mirrors `GET /entries/manifest` on the server: keyword, kind and time-range
 * filters, then a page of the matches. An absent `page` returns every match.
 */
export type EntriesManifestFilter = {
  query?: string;
  kind?: EntryFilter;
  start?: number;
  end?: number;
  page?: number;
};

export type EntriesManifestPage = {
  total: number;
  entries: LocalClipboardEntry[];
};
export type UploadProgress = { uploadedBytes: number; totalBytes: number };
export type DownloadProgress = { finished: number; total: number };
export type EntryFilter = "all" | ClipboardKind;
export type TimeFilter = "all" | "today" | "7-days" | "30-days" | "custom";
export type SettingsPage = "general" | "shortcuts" | "account" | "data" | "about";
export type ToastTone = "success" | "error" | "info";
export type ToastPayload = { message: string; tone: ToastTone };
export type ShareReceiverEvent = { id?: string; error?: string };
export type ShareImportSummary = { shares: number; texts: number; images: number; files: number };
