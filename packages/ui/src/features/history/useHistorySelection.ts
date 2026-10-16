import { computed, ref, watch, type Ref } from "vue";

/** Selection is limited to visible ids and independent of keyboard/hover focus. */
export function useHistorySelection(visibleIds: Readonly<Ref<readonly string[]>>) {
  const selecting = ref(false);
  const selectedIds = ref(new Set<string>());
  let anchor = "";
  const allSelected = computed(() => visibleIds.value.length > 0 && visibleIds.value.every((id) => selectedIds.value.has(id)));

  function clear(): void {
    selecting.value = false;
    selectedIds.value = new Set();
    anchor = "";
  }

  function start(id?: string): void {
    selecting.value = true;
    const visible = id && visibleIds.value.includes(id) ? id : "";
    selectedIds.value = new Set(visible ? [visible] : []);
    anchor = visible;
  }

  function toggle(id: string, range = false, additive = false): void {
    if (!visibleIds.value.includes(id)) return;
    selecting.value = true;
    const ids = visibleIds.value;
    const from = ids.indexOf(anchor), to = ids.indexOf(id);
    if (range && from >= 0 && to >= 0) {
      const selection = ids.slice(Math.min(from, to), Math.max(from, to) + 1);
      selectedIds.value = new Set(additive ? [...selectedIds.value, ...selection] : selection);
    } else {
      const next = new Set(selectedIds.value);
      if (next.has(id)) next.delete(id); else next.add(id);
      selectedIds.value = next;
      anchor = id;
    }
  }

  function toggleAll(): void {
    selecting.value = true;
    selectedIds.value = new Set(allSelected.value ? [] : visibleIds.value);
    anchor = visibleIds.value[0] ?? "";
  }

  function setAnchor(id: string): void { anchor = id; }

  watch(visibleIds, (ids) => {
    const visible = new Set(ids);
    selectedIds.value = new Set([...selectedIds.value].filter((id) => visible.has(id)));
    if (!visible.has(anchor)) anchor = "";
  });

  return { selecting, selectedIds, allSelected, start, toggle, toggleAll, clear, setAnchor };
}
