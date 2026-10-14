import { entryContents, isFileNode, type FileInfo, type TreeNode } from "@cliproam/protocol";
import type { ClipboardEntry, Device, EntrySummary, LocalClipboardEntry, StoredEntry } from "./types";

// 取消信号

/** Combine cancellation signals on WebViews that lack AbortSignal.any. */
export function anyAbortSignal(signals: AbortSignal[]): AbortSignal {
  if (typeof AbortSignal.any === "function") return AbortSignal.any(signals);

  const controller = new AbortController();
  const alreadyAborted = signals.find((signal) => signal.aborted);
  if (alreadyAborted) {
    controller.abort(alreadyAborted.reason);
    return controller.signal;
  }

  const listeners = new Map<AbortSignal, () => void>();
  for (const signal of new Set(signals)) {
    const onAbort = () => {
      controller.abort(signal.reason);
      for (const [source, listener] of listeners) {
        source.removeEventListener("abort", listener);
      }
      listeners.clear();
    };
    listeners.set(signal, onAbort);
    signal.addEventListener("abort", onAbort, { once: true });
  }
  return controller.signal;
}

// 字节转换

export function base64ToBytes(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 8192) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
  }
  return btoa(binary);
}

// 错误信息

/** The message of an unknown thrown value, for toasts and error strings. */
export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

// 日期、大小与进度格式化

/** Pure date/size formatting helpers shared by every window. */

/** Labels for the preset time filters; TimeFilterControl renders the same strings. */
export const TIME_FILTER_LABELS: Record<string, string> = {
  today: "今天",
  "7-days": "近 7 天",
  "30-days": "近 30 天",
};

export function formatAge(createdAt: string, now: number): string {
  const elapsed = Math.max(0, now - new Date(createdAt).getTime());
  const seconds = Math.floor(elapsed / 1_000);
  if (seconds < 10) return "刚刚";
  if (seconds < 60) return `${seconds} 秒前`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  return new Intl.DateTimeFormat("zh-CN", { month: "short", day: "numeric" }).format(new Date(createdAt));
}

export function formatExactDateTime(createdAt: string | number): string {
  const date = new Date(createdAt);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

export function parseLocalDate(value: string, endOfDay = false): Date | undefined {
  const [year, month, day] = value.split("-").map(Number);
  if (!year || !month || !day) return undefined;
  return new Date(year, month - 1, day, endOfDay ? 23 : 0, endOfDay ? 59 : 0, endOfDay ? 59 : 0, endOfDay ? 999 : 0);
}

/** Shared custom-date-range validation; returns the error message, or "" when valid. */
export function validateDateRange(start: string, end: string): string {
  if (!start || !end) return "请选择完整的开始和结束日期";
  if (start > end) return "开始日期不能晚于结束日期";
  return "";
}

// 与 apps/admin/src/sections/files/FilesSection.vue 的 formatBytes 是同一实现，改动请同步。
export function formatFileSize(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

/** cur/total 的整数百分比，clamp 到 cap（进行中默认 99 封顶，到位由终态接管）；total 无效时为 0。 */
export function percentOf(cur: number, total: number, cap = 100): number {
  if (!total) return 0;
  return Math.min(cap, Math.floor((cur / total) * 100));
}

// 剪贴板条目展示

const MANUAL_UPLOAD_LIMIT = 100 * 1024 * 1024;

/**
 * Pure display helpers shared by the history and pending-upload views. Anything
 * that reads reactive state takes it as an argument so both views can feed it
 * from their own props.
 */

export function deviceName(devicesById: Record<string, Device>, entry: ClipboardEntry): string {
  return devicesById[entry.sourceDeviceId]?.name ?? "";
}

export function isHashing(entry: LocalClipboardEntry): boolean {
  return entry.summary.hashedCount < entry.summary.fileCount;
}

/**
 * Upload state is derived from the entry's summary: `storedCount` comes from
 * the local `files` table (persisted, backfilled from `/files/query`), so no
 * live server state rides in here.
 */
export function uploadStatus(
  entry: LocalClipboardEntry,
): string | undefined {
  const summary = entry.summary;
  if (!summary.fileCount) return undefined;
  // Content ids are computed in the background, so a fresh entry is usable
  // locally before it can be addressed on the server.
  if (isHashing(entry)) return `计算中 ${summary.hashedCount}/${summary.fileCount}`;
  if (!summary.contentCount) return undefined;
  const storedCount = summary.storedCount;
  if (storedCount >= summary.contentCount) return "已上传";
  if (storedCount) {
    return `部分上传（${storedCount}/${summary.contentCount}）`;
  }
  if (summary.uploadableSize !== undefined && summary.uploadableSize >= MANUAL_UPLOAD_LIMIT) {
    return "未上传（超过 100 MB）";
  }
  return "未上传";
}

export function fileEntrySummary(entry: LocalClipboardEntry): string | undefined {
  if (entry.kind !== "files" || !entry.summary.fileCount) return undefined;
  const count = `${entry.summary.fileCount} 个文件`;
  if (!entry.summary.totalSize) return count;
  return `${count} · ${formatFileSize(entry.summary.totalSize)}`;
}

export function canSaveEntry(entry: LocalClipboardEntry): boolean {
  return (entry.kind === "files" || entry.kind === "image")
    && entry.summary.contentCount > 0;
}

export function saveEntryLabel(entry: LocalClipboardEntry, savingEntryId: string): string {
  return savingEntryId === entry.id ? "正在下载…" : "下载";
}

// 文件名称与剪贴板内容摘要

export function safeName(name: string): string {
  const value = name.replace(/[<>:"/\\|?*\x00-\x1f]/g, "_").replace(/[. ]+$/, "");
  if (!value || value === "." || value === "..") return "unnamed";
  return value;
}

function leaves(info: FileInfo): { path: string; fileId: string; size: number }[] {
  const values: { path: string; fileId: string; size: number }[] = [];
  const walk = (dir: FileInfo, prefix: string) => {
    for (const [name, node] of Object.entries(dir)) {
      if (safeName(name) !== name) throw new Error(`文件名称不合法：${name}`);
      const path = prefix ? `${prefix}/${name}` : name;
      if (isFileNode(node)) values.push({ path, fileId: node.f, size: node.s });
      else walk(node, path);
    }
  };
  walk(info, "");
  return values;
}

export function nameMatches(name: string, patterns: string[]): boolean {
  return patterns.some((pattern) => {
    const escaped = pattern.trim().replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".");
    return new RegExp(`^${escaped}$`, "iu").test(name);
  });
}

export async function summarize(
  entry: StoredEntry,
  stored: ReadonlySet<string>,
  resolve: (fileId: string) => Promise<string | undefined>,
  lightweight = false,
): Promise<LocalClipboardEntry> {
  const content = entryContents(entry).filter((file) => file.fileId);
  const fileLeaves = entry.fileInfo ? leaves(entry.fileInfo) : [];
  const roots: TreeNode[] = Object.values(entry.fileInfo ?? {});
  const rootKinds = new Set(roots.map((node) => isFileNode(node) ? "file" : "dir"));
  const summary: EntrySummary = {
    rootKind: rootKinds.size > 1 ? "mixed" : [...rootKinds][0] ?? "",
    fileCount: entry.kind === "image" ? 1 : fileLeaves.length,
    hashedCount: entry.kind === "image" ? 1 : fileLeaves.filter((file) => file.fileId).length,
    contentCount: content.length, totalSize: 0, maxFileSize: 0,
    readyCount: 0, pendingCount: 0, pendingSize: 0, storedCount: 0,
  };
  for (const file of content) {
    summary.totalSize += file.size;
    summary.maxFileSize = Math.max(summary.maxFileSize, file.size);
    if (stored.has(file.fileId)) summary.storedCount++;
    const path = await resolve(file.fileId);
    if (path) {
      summary.readyCount++;
      summary.uploadableSize = Math.min(summary.uploadableSize ?? Infinity, file.size);
      if (entry.kind === "image") summary.previewPath = path;
    } else { summary.pendingCount++; summary.pendingSize += file.size; }
  }
  const result = { ...entry, summary };
  if (lightweight) delete result.fileInfo;
  return result;
}

// 哈希与账号目录名称

/** Incremental SHA-256: bounded memory for uploads, downloads and large files. */
const K = new Uint32Array([
  0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
  0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
  0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
  0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
  0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
  0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
  0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
  0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2,
]);
const rotate = (n: number, bits: number) => (n >>> bits) | (n << (32 - bits));

export class Sha256 {
  #state = new Uint32Array([0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19]);
  #buffer = new Uint8Array(64);
  #used = 0;
  #length = 0;
  #words = new Uint32Array(64);

  update(bytes: Uint8Array): this {
    this.#length += bytes.length;
    let offset = 0;
    while (offset < bytes.length) {
      const count = Math.min(64 - this.#used, bytes.length - offset);
      this.#buffer.set(bytes.subarray(offset, offset + count), this.#used);
      this.#used += count; offset += count;
      if (this.#used === 64) { this.#block(); this.#used = 0; }
    }
    return this;
  }

  #block(): void {
    const w = this.#words;
    const view = new DataView(this.#buffer.buffer);
    for (let i = 0; i < 16; i++) w[i] = view.getUint32(i * 4);
    for (let i = 16; i < 64; i++) {
      const a = w[i - 15]!, b = w[i - 2]!;
      w[i] = (rotate(a,7) ^ rotate(a,18) ^ (a >>> 3)) + w[i - 16]! + (rotate(b,17) ^ rotate(b,19) ^ (b >>> 10)) + w[i - 7]!;
    }
    let [a,b,c,d,e,f,g,h] = [...this.#state] as [number,number,number,number,number,number,number,number];
    for (let i = 0; i < 64; i++) {
      const t1 = (h + (rotate(e,6)^rotate(e,11)^rotate(e,25)) + ((e&f)^(~e&g)) + K[i]! + w[i]!) >>> 0;
      const t2 = ((rotate(a,2)^rotate(a,13)^rotate(a,22)) + ((a&b)^(a&c)^(b&c))) >>> 0;
      h=g; g=f; f=e; e=(d+t1)>>>0; d=c; c=b; b=a; a=(t1+t2)>>>0;
    }
    [a,b,c,d,e,f,g,h].forEach((value, i) => { this.#state[i] = (this.#state[i]! + value) >>> 0; });
  }

  digest(): string {
    const bits = BigInt(this.#length) * 8n;
    this.update(new Uint8Array([128]));
    while (this.#used !== 56) this.update(new Uint8Array([0]));
    const tail = new Uint8Array(8);
    new DataView(tail.buffer).setBigUint64(0, bits);
    this.update(tail);
    return [...this.#state].map((value) => value.toString(16).padStart(8, "0")).join("");
  }
}

export function fnv1a(value: string): string {
  let hash = 0xcbf29ce484222325n;
  for (const byte of new TextEncoder().encode(value)) hash = BigInt.asUintN(64, (hash ^ BigInt(byte)) * 0x100000001b3n);
  return hash.toString(16).padStart(16, "0");
}

export function profileName(account: string): string {
  const name = [...account.replace(/^account:/, "")].map((c) => /^[a-z0-9._-]$/i.test(c) ? c : "_").join("") || "local";
  return `${name}-${fnv1a(account)}`;
}
