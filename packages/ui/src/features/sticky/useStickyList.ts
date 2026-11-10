import { computed, nextTick, onScopeDispose, ref, watch } from "vue";
import { getAccountSession } from "../../accountSession";
import { errorMessage } from "../../utils";
import type { ManifestOptions } from "../history/useHistoryManifest";

/** The page filters the same complete collection used by desktop windows. */
export function useStickyList(options: ManifestOptions) {
  const history = getAccountSession()?.history;
  const page = ref(1);
  const pageCount = ref(1);
  const cleared = ref(false);
  let token = 0;
  const entries = computed(() => {
    if (cleared.value) return [];
    const filter = options.buildFilter(1);
    const query = filter.query?.trim().toLowerCase();
    return (history?.stickyEntries.value ?? []).filter((entry) => {
      const time = Date.parse(entry.createdAt);
      return (!query || entry.content.toLowerCase().includes(query))
        && (!filter.kind || filter.kind === "all" || entry.kind === filter.kind)
        && (filter.start === undefined || time >= filter.start)
        && (filter.end === undefined || time <= filter.end)
        && (!filter.deviceIds?.length || filter.deviceIds.includes(entry.sourceDeviceId));
    });
  });
  const total = computed(() => entries.value.length);

  watch(entries, (items) => {
    if (!items.some((entry) => entry.id === options.getSelectedEntryId())) {
      options.setSelectedEntryId(items[0]?.id ?? "");
    }
  }, { immediate: true });

  async function fetch(_page = 1, scrollToTop = false): Promise<void> {
    if (options.canFetch && !options.canFetch()) return;
    cleared.value = false;
    const current = ++token;
    try {
      await history?.listSticky();
      if (current !== token) return;
      if (scrollToTop) {
        await nextTick();
        if (options.listElement?.value) options.listElement.value.scrollTop = 0;
      }
    } catch (error) {
      if (current === token) options.onError?.(errorMessage(error));
    }
  }
  const cancel = () => { token++; };
  onScopeDispose(cancel);
  watch(options.filterSources, () => { void fetch(1, true); });
  return {
    page, pageCount, entries, total, fetch, cancel,
    clear: () => { cancel(); cleared.value = true; },
    changePage: async (_page: number) => {},
    refresh: () => fetch(),
  };
}
