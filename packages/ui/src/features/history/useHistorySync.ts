import { computed, nextTick, onActivated, onBeforeUnmount, onDeactivated, onMounted, ref, watch } from "vue";
import type { ClipboardEntry } from "@cliproam/protocol";
import { getAccountSession } from "../../accountSession";
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

/** Subscribe only while mounted, activated by KeepAlive, and visible (including v-show). */
export function useHistoryEvents(onRefresh: () => void, enabled: () => boolean = () => true) {
  const activated = ref(false);
  const active = computed(() => activated.value && enabled());
  watch(active, (visible, _previous, onCleanup) => {
    if (!visible) return;
    const history = getAccountSession()?.history;
    if (!history) return;
    history.addEvent(onRefresh);
    onCleanup(() => history.delEvent(onRefresh));
    // Changes while away need no replay: query the current server state on return.
    onRefresh();
  }, { flush: "sync" });
  onMounted(() => { activated.value = true; });
  onActivated(() => { activated.value = true; });
  onDeactivated(() => { activated.value = false; });
  onBeforeUnmount(() => { activated.value = false; });
  return active;
}

export async function fetchHistoryPage(
  filter: EntriesManifestFilter,
): Promise<EntriesManifestPage> {
  return getAccountSession()?.history.fetchHistoryPage(filter) ?? { total: 0, entries: [] };
}

/** Notify the currently subscribed history view. */
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
