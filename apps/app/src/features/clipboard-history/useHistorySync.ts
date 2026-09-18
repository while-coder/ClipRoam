import { nextTick, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { ClipboardEntry } from "@cliproam/protocol";
import { activeView } from "../../composables/useActiveView";
import { showToast } from "../toast/useToast";
import { errorMessage } from "../../utils/error";
import { getActiveConfig } from "../sync/syncSession";
import type { SyncClient } from "../sync/syncClient";
import type {
  EntriesManifestFilter,
  EntriesManifestPage,
} from "../../types";

/**
 * 历史数据的同步视图状态（从 App.vue 下沉）：revision 失效、远端 upsert
 * 批量合并、服务器文件可用性对账。引擎客户端与 pending 刷新经 App.vue
 * 装配时注入，保持静态依赖单向。
 */
/** HistoryView 暴露实例的最小面（defineExpose({ handleKeydown, focusSearch, focusSearchInput, currentPage })）。 */
interface HistorySyncViewRef {
  currentPage?: number;
  focusSearch(): Promise<void>;
  focusSearchInput(): void;
}

interface HistorySyncDeps {
  getSyncClient(): SyncClient | undefined;
  refreshPendingCount(): Promise<void>;
  refreshPendingEntries(): Promise<void>;
  getHistoryView(): HistorySyncViewRef | undefined;
}

let deps: HistorySyncDeps;

export function initHistorySync(next: HistorySyncDeps): void {
  deps = next;
}

/** Bumped whenever the history may have changed; the history view refetches its page on it. */
export const historyRevision = ref(0);
export const syncedEntryIds = ref(new Set<string>());

export async function fetchManifest(
  filter: EntriesManifestFilter,
  deviceNames: Record<string, string>,
): Promise<EntriesManifestPage> {
  // 未登录没有活动档案；隐藏的 paste 窗口启动时也会来查，这里返回空页，
  // 不让「同步账号未登录」的错误以 toast 形式盖到主窗口的登录页上。
  if (!getActiveConfig()?.sessionToken) {
    return { total: 0, entries: [] };
  }
  return invoke<EntriesManifestPage>("list_entries_manifest", { filter, deviceNames });
}

let refreshTimer: number | undefined;

/**
 * Background events (captures, remote upserts, file availability) arrive in
 * bursts; each one only invalidates views. A burst coalesces into one pass:
 * the history view refetches its current page on the revision bump, while the
 * pending badge (and, when its view is open, the queue details) re-query
 * Rust-side. Nothing reads whole history.
 */
export function refreshHistory(): void {
  if (refreshTimer !== undefined) return;
  refreshTimer = window.setTimeout(() => {
    refreshTimer = undefined;
    historyRevision.value += 1;
    void deps.refreshPendingCount();
    if (activeView.value === "pending-sync") void deps.refreshPendingEntries();
    void syncFileStatuses();
  }, 200);
}

export function cancelRefreshBurst(): void {
  if (refreshTimer !== undefined) window.clearTimeout(refreshTimer);
}

/**
 * Server-pool availability is persisted in the local `files` table; this only
 * fills its gaps. Rust reports the content ids without a confirmed pool
 * answer, one batched `/files/query` answers them, and the result lands in
 * the table where the summaries read it. Steady state returns an empty list,
 * so it costs nothing; a failure just leaves the gap for the next pass.
 * `recheckUnstored` (reconnect only) also re-asks ids last answered "not
 * stored", healing a `file.available` push missed while offline.
 */
export async function syncFileStatuses(recheckUnstored = false): Promise<void> {
  const client = deps.getSyncClient();
  if (!client) return;
  try {
    const fileIds = await invoke<string[]>("find_unknown_file_ids", { recheckUnstored });
    if (!fileIds.length) return;
    const statuses = await client.fetchFiles(fileIds);
    await invoke("upsert_server_files", { statuses });
    // The summaries changed, so the history view refetches its current page.
    historyRevision.value += 1;
  } catch {
    // Auxiliary display state; the next refresh retries.
  }
}

const pendingRemoteUpserts = new Map<string, ClipboardEntry>();
let remoteUpsertFlush: Promise<void> | undefined;

/**
 * Whether the history view needs a visible refresh for a newly arrived entry.
 * The entry lands on page 1, so only a view sitting there must refetch; deeper
 * pages keep their scroll (their slice shifts by one and the next page change
 * or revision bump catches up), and an unmounted view refetches page 1 on
 * remount anyway.
 */
function historyOnFirstPage(): boolean {
  return deps.getHistoryView()?.currentPage === 1;
}

/**
 * Remote entry echoes arrive one per published entry, but each write rewrites
 * the durable history. Queue them so a burst becomes a single batch command.
 */
export function queueRemoteUpsert(entry: ClipboardEntry): Promise<void> {
  pendingRemoteUpserts.set(entry.id, entry);
  if (remoteUpsertFlush) return remoteUpsertFlush;
  remoteUpsertFlush = new Promise<void>((resolve) => {
    window.setTimeout(() => {
      const batch = [...pendingRemoteUpserts.values()];
      pendingRemoteUpserts.clear();
      remoteUpsertFlush = undefined;
      void applyRemoteUpserts(batch, historyOnFirstPage()).finally(resolve);
    }, 200);
  });
  return remoteUpsertFlush;
}

export async function applyRemoteUpserts(batch: ClipboardEntry[], refresh = true): Promise<void> {
  markEntriesSynced(batch);
  try {
    await invoke("upsert_server_entries", { entries: batch });
  } catch (error) {
    showToast(`写入同步记录失败：${errorMessage(error)}`, "error");
    return;
  }
  if (refresh) refreshHistory();
}

/** 批量标记已同步：一次构建新 Set，避免逐条 clone 整个集合（burst 推送时是 O(n²)）。 */
function markEntriesSynced(entries: readonly ClipboardEntry[]): void {
  if (!entries.length) return;
  const known = new Set(syncedEntryIds.value);
  let changed = false;
  for (const entry of entries) {
    if (!known.has(entry.id)) {
      known.add(entry.id);
      changed = true;
    }
  }
  if (changed) syncedEntryIds.value = known;
}

export function focusSearch(): void {
  void nextTick(() => deps.getHistoryView()?.focusSearch());
}

/** 只把焦点还给搜索框：不清搜索词、不重拉第 1 页（关弹窗、启动完成用）。 */
export function focusSearchInput(): void {
  void nextTick(() => deps.getHistoryView()?.focusSearchInput());
}

/**
 * The rendered list carries only aggregates. Publishing needs the directory tree,
 * so it is fetched per entry instead of for the whole history.
 */
export async function fullEntry(entry: Pick<ClipboardEntry, "id">): Promise<ClipboardEntry> {
  return invoke<ClipboardEntry>("get_entry", { entryId: entry.id });
}

/** 卸载兜底：未落盘的远端 upsert 立即写掉（fire-and-forget，不 await）。 */
export function flushPendingRemoteUpserts(): void {
  if (pendingRemoteUpserts.size) {
    void applyRemoteUpserts([...pendingRemoteUpserts.values()]);
    pendingRemoteUpserts.clear();
  }
}
