import { nextTick, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { ClipboardEntry } from "@cliproam/protocol";
import { activeView } from "../../composables/useActiveView";
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

/** First-page convenience entry; filtering and caching use the same page flow. */
export function fetchManifest(
  filter: EntriesManifestFilter,
  deviceNames: Record<string, string>,
): Promise<EntriesManifestPage> {
  return fetchHistoryPage({ ...filter, page: 1 }, deviceNames);
}

export async function fetchHistoryPage(
  filter: EntriesManifestFilter,
  deviceNames: Record<string, string>,
): Promise<EntriesManifestPage> {
  // 未登录没有活动档案；隐藏的 paste 窗口启动时也会来查，这里返回空页，
  // 不让「同步账号未登录」的错误以 toast 形式盖到主窗口的登录页上。
  if (!getActiveConfig()?.sessionToken) {
    return { total: 0, entries: [] };
  }
  const client = deps.getSyncClient();
  if (!client) return { total: 0, entries: [] };
  return client.fetchHistoryPage(filter, deviceNames);
}

let refreshTimer: number | undefined;

/**
 * Background events (captures, remote notifications, file availability) arrive in
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
  }, 200);
}

export function cancelRefreshBurst(): void {
  if (refreshTimer !== undefined) window.clearTimeout(refreshTimer);
}

export function focusSearch(): void {
  void nextTick(() => deps.getHistoryView()?.focusSearch());
}

/** 只把焦点还给搜索框：不清搜索词、不重拉第 1 页（关弹窗、启动完成用）。 */
export function focusSearchInput(): void {
  void nextTick(() => deps.getHistoryView()?.focusSearchInput());
}

/**
 * Read full details already cached by backfill; rendered lists omit directory trees.
 */
export async function fullEntry(entry: Pick<ClipboardEntry, "id">): Promise<ClipboardEntry> {
  return invoke<ClipboardEntry>("get_entry", { entryId: entry.id });
}
