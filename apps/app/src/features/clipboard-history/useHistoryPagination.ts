import { computed, nextTick, ref, watch, type Ref, type WatchSource } from "vue";
import { PAGE_SIZE } from "../../utils/constants";
import type { LocalClipboardEntry } from "../../types";

type PaginationOptions = {
  listElement?: Ref<HTMLElement | undefined>;
  getSelectedEntryId: () => string;
  setSelectedEntryId: (id: string) => void;
};

/**
 * Client-side paging over the filtered history. The backend still returns the
 * whole history in one round-trip; paging only bounds what the list renders.
 */
export function useHistoryPagination(
  filteredEntries: Ref<LocalClipboardEntry[]>,
  filterSources: WatchSource[],
  options: PaginationOptions,
) {
  const page = ref(1);
  const pageCount = computed(() => Math.max(1, Math.ceil(filteredEntries.value.length / PAGE_SIZE)));
  const pagedEntries = computed(() =>
    filteredEntries.value.slice((page.value - 1) * PAGE_SIZE, page.value * PAGE_SIZE),
  );

  async function goToPage(next: number): Promise<void> {
    page.value = Math.min(Math.max(1, next), pageCount.value);
    await nextTick();
    if (options.listElement?.value) options.listElement.value.scrollTop = 0;
  }

  /** Jumps to the page holding a filtered-list index — keyboard navigation can cross pages. */
  function goToPageOf(index: number): void {
    const targetPage = Math.floor(index / PAGE_SIZE) + 1;
    if (targetPage !== page.value) void goToPage(targetPage);
  }

  /** Page-button navigation: the selection follows so Enter always acts on a visible entry. */
  async function changePage(next: number): Promise<void> {
    if (next === page.value) return;
    await goToPage(next);
    if (!pagedEntries.value.some((entry) => entry.id === options.getSelectedEntryId())) {
      options.setSelectedEntryId(pagedEntries.value[0]?.id ?? "");
    }
  }

  // Only user-facing filters reset the page. Watching the filtered list itself
  // would yank the user back to page 1 on every background history refresh.
  watch(filterSources, () => { page.value = 1; });
  watch(pageCount, () => {
    if (page.value > pageCount.value) page.value = pageCount.value;
  });

  return { page, pageCount, pagedEntries, goToPage, goToPageOf, changePage };
}
