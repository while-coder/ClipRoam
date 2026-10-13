import { computed, nextTick } from "vue";
import type { ClipboardEntry } from "@cliproam/protocol";
import { getAccountSession } from "../../core/accountSession";
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
  getHistoryView(): HistorySyncViewRef | undefined;
}

let deps: HistorySyncDeps;

export function initHistorySync(next: HistorySyncDeps): void {
  deps = next;
}

/** Bumped whenever the history may have changed; the history view refetches its page on it. */
export const historyRevision = computed(() => getAccountSession()?.history.revision.value ?? 0);

export async function fetchHistoryPage(
  filter: EntriesManifestFilter,
): Promise<EntriesManifestPage> {
  return getAccountSession()?.history.fetchHistoryPage(filter) ?? { total: 0, entries: [] };
}

/** Mark history changed immediately; only a mounted history view fetches it. */
export function refreshHistory(): void {
  getAccountSession()?.history.invalidate();
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
  const session = getAccountSession();
  if (!session) throw new Error("同步账号未登录");
  return session.history.getEntry(entry.id);
}
