import { computed, nextTick, ref, watch, type Ref, type WatchSource } from "vue";
import { PAGE_SIZE } from "../../utils/constants";
import type { EntriesManifestFilter, EntriesManifestPage, LocalClipboardEntry } from "../../types";

type ManifestOptions = {
  fetchManifest: (
    filter: EntriesManifestFilter,
    deviceNames: Record<string, string>,
  ) => Promise<EntriesManifestPage>;
  deviceNames: () => Record<string, string>;
  buildFilter: (page: number) => EntriesManifestFilter;
  /** Bumped whenever the underlying history may have changed in the background. */
  revision: Ref<number>;
  /** Filter inputs: watching them re-runs the query from page 1. */
  filterSources: WatchSource[];
  listElement?: Ref<HTMLElement | undefined>;
  getSelectedEntryId: () => string;
  setSelectedEntryId: (id: string) => void;
};

/**
 * Server-style manifest paging: the backend filters, counts and slices, so
 * only the rendered page ever crosses the IPC boundary. `revision` bumps keep
 * the current page in place; filter edits restart from page 1.
 */
export function useHistoryManifest(options: ManifestOptions) {
  const page = ref(1);
  const total = ref(0);
  const entries = ref<LocalClipboardEntry[]>([]);
  const loading = ref(false);
  let fetchToken = 0;

  const pageCount = computed(() => Math.max(1, Math.ceil(total.value / PAGE_SIZE)));

  async function fetch(requestedPage: number, scrollToTop = false): Promise<void> {
    const token = ++fetchToken;
    loading.value = true;
    try {
      const result = await options.fetchManifest(
        options.buildFilter(requestedPage),
        options.deviceNames(),
      );
      if (token !== fetchToken) return;
      total.value = result.total;
      entries.value = result.entries;
      // The history can shrink between the request and the response; land on
      // the last valid page instead of showing an empty slice.
      const clamped = Math.min(Math.max(1, requestedPage), pageCount.value);
      page.value = clamped;
      if (clamped !== requestedPage) {
        await fetch(clamped);
        return;
      }
      if (!entries.value.some((entry) => entry.id === options.getSelectedEntryId())) {
        options.setSelectedEntryId(entries.value[0]?.id ?? "");
      }
      if (scrollToTop) {
        await nextTick();
        if (options.listElement?.value) options.listElement.value.scrollTop = 0;
      }
    } finally {
      if (token === fetchToken) loading.value = false;
    }
  }

  /** Drops in-flight fetches and shows an empty list — used for invalid filters. */
  function clear(): void {
    fetchToken += 1;
    total.value = 0;
    entries.value = [];
    loading.value = false;
  }

  watch(options.filterSources, () => { void fetch(1, true); });
  // `immediate` covers the first mount and re-mounts of the view; later bumps
  // refresh the current page in the background.
  watch(options.revision, () => { void fetch(page.value); }, { immediate: true });

  async function changePage(next: number): Promise<void> {
    if (next === page.value) return;
    await fetch(next, true);
  }

  return { page, total, pageCount, entries, loading, fetch, clear, changePage };
}
