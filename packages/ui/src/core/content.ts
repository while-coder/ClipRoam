import { entryContents, isFileNode, type ClipboardEntry, type FileInfo, type TreeNode } from "@cliproam/protocol";
import type { EntrySummary, LocalClipboardEntry } from "../types";

export type LocalSource = { path: string; source: string; size: number; modifiedAt?: number; fileId?: string };
export type LocalSources = { roots: string[]; files: LocalSource[] };
export type StoredEntry = ClipboardEntry & { sources?: LocalSources; pendingStatus?: "wait" | "failed" };
export type PendingRow = { seq: number; status: "wait" | "failed"; entry: StoredEntry; captureId?: string; resolving?: boolean };

export function safeName(name: string): string {
  const value = name.replace(/[<>:"/\\|?*\x00-\x1f]/g, "_").replace(/[. ]+$/, "");
  if (!value || value === "." || value === "..") return "unnamed";
  return value;
}

export function leaves(info: FileInfo): { path: string; fileId: string; size: number }[] {
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
