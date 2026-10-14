import type {
  ClipboardEntry,
  ClipboardKind,
  Device,
} from "@cliproam/protocol";

export type { ClipboardEntry, ClipboardKind, Device };

/** 会话凭证：全局唯一，决定激活哪个账号档案。 */
export type SyncConfig = {
  serverAddress: string;
  serverProtocol: "http" | "https";
  username: string;
  sessionToken: string;
};

/** 账号偏好：跟随历史档案存储，换账号登录互不污染。 */
export type AccountPreferences = {
  autoUploadLimitMb: number;
  autoReceiveClipboard: boolean;
  /** 是否监听本机剪贴板（自动捕获复制内容进历史）；关闭后仅接收同步内容。 */
  watchClipboard: boolean;
  excludePatterns: string[];
  /** 登录时服务器下发的单文件存储上限（MB），自动上传档位不超过它。 */
  serverMaxFileMb: number;
  /** 登录时服务器下发的单次复制文件数上限，捕获时超过则不捕获、不同步。 */
  maxCaptureFileCount: number;
};

export type SavePreparation = { saveId: string };

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
 * Aggregates computed by the TS cache. A folder can hold thousands of nodes, so
 * the list never receives the tree itself — only these counters.
 */
export type EntrySummary = {
  rootKind: string;
  fileCount: number;
  hashedCount: number;
  contentCount: number;
  totalSize: number;
  maxFileSize: number;
  readyCount: number;
  pendingCount: number;
  pendingSize: number;
  /** Contents the server pool holds (the local `files` table, `stored = 1`). */
  storedCount: number;
  uploadableSize?: number;
  previewPath?: string;
};

export type LocalClipboardEntry = ClipboardEntry & { summary: EntrySummary; pendingStatus?: "wait" | "failed" };

/**
 * History page filters. Page numbers start at 1 in the app, HTTP and local
 * cache queries.
 */
export type EntriesManifestFilter = {
  query?: string;
  kind?: EntryFilter;
  start?: number;
  end?: number;
  /** 来源设备过滤；空/缺省 = 不过滤。 */
  deviceIds?: string[];
  /** 1-based page number; omitted means the first page for server requests. */
  page?: number;
};

export type EntriesManifestPage = {
  total: number;
  entries: LocalClipboardEntry[];
};
export type UploadProgress = { uploadedBytes: number; totalBytes: number };
export type EntryFilter = "all" | ClipboardKind;
export type TimeFilter = "all" | "today" | "7-days" | "30-days" | "custom";
export type SettingsPage = "general" | "shortcuts" | "account" | "data" | "about";
export type NotificationTone = "success" | "error" | "info";
export type NotificationOptions = { message: string; title?: string; tone?: NotificationTone };
export type NotificationPayload = { message: string; title: string; tone: NotificationTone };
export type ShareReceiverEvent = { id?: string; error?: string };
export type ShareImportSummary = { shares: number; texts: number; images: number; files: number };

export type WorkspaceView = "history" | "pending-upload" | "uploads" | "downloads" | "devices";

export type LocalSource = { path: string; source: string; size: number; modifiedAt?: number; fileId?: string };
export type LocalSources = { roots: string[]; files: LocalSource[] };
export type StoredEntry = ClipboardEntry & { sources?: LocalSources; pendingStatus?: "wait" | "failed" };
export type PendingRow = { seq: number; status: "wait" | "failed"; entry: StoredEntry; captureId?: string; resolving?: boolean };
