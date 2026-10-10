import { nextTick } from "vue";
import type { ClipboardEntry } from "@cliproam/protocol";
import { accountStateRef, getAccountSession } from "../sync/accountSession";
import { getActiveConfig } from "../sync/syncSession";
import type { SyncClient } from "../sync/syncClient";
import type {
  EntriesManifestFilter,
  EntriesManifestPage,
} from "../../types";

/** History refresh state and view focus helpers. */
interface HistorySyncViewRef {
  focusSearch(): Promise<void>;
  focusSearchInput(): void;
}

interface HistorySyncDeps {
  getSyncClient(): SyncClient | undefined;
  getHistoryView(): HistorySyncViewRef | undefined;
}

let deps: HistorySyncDeps;

export function initHistorySync(next: HistorySyncDeps): void {
  deps = next;
}

/** Bumped whenever the history may have changed; the history view refetches its page on it. */
export const historyRevision = accountStateRef("historyRevision");

export async function fetchHistoryPage(
  filter: EntriesManifestFilter,
): Promise<EntriesManifestPage> {
  // 未登录没有活动档案；隐藏的 paste 窗口启动时也会来查，这里返回空页，
  // 不让「同步账号未登录」的错误以 toast 形式盖到主窗口的登录页上。
  if (!getActiveConfig()?.sessionToken) {
    return { total: 0, entries: [] };
  }
  const client = deps.getSyncClient();
  if (!client) return { total: 0, entries: [] };
  return client.history.fetchHistoryPage(filter);
}

/** Coalesce refreshes inside the account lifetime. */
export function refreshHistory(): void {
  const session = getAccountSession();
  session?.schedule("history-refresh", () => { session.state.historyRevision.value += 1; }, 200);
}
export function cancelRefreshBurst(): void { getAccountSession()?.cancelScheduled("history-refresh"); }

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
  const client = deps.getSyncClient();
  if (!client) throw new Error("同步账号未登录");
  return client.history.getEntry(entry.id);
}
