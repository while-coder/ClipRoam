<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { Copy, Download, LoaderCircle, Trash2 } from "lucide-vue-next";
import { canSaveEntry, saveEntryLabel } from "../../utils/entry";
import type { LocalClipboardEntry } from "../../types";

/**
 * 历史条目的右键/长按菜单（复制、另存为、删除）。定位来自触发点
 * 坐标而不是 trigger 元素；骨架（Teleport + fixed + 外点关闭 + 键盘导航）
 * 与 DeviceFilterControl 相同。
 */
const props = defineProps<{
  entry: LocalClipboardEntry;
  x: number;
  y: number;
  activating: boolean;
  savingEntryId: string;
  isMobile: boolean;
}>();

const emit = defineEmits<{
  activate: [entry: LocalClipboardEntry];
  save: [entry: LocalClipboardEntry];
  remove: [entry: LocalClipboardEntry];
  close: [];
}>();

const menu = ref<HTMLElement>();
const menuStyle = ref<Record<string, string>>({});
const saving = computed(() => props.savingEntryId === props.entry.id);
const saveLabel = computed(() => saveEntryLabel(props.entry, props.savingEntryId, props.isMobile));

// 右键点可能贴近屏幕右/下边缘，先按(宽, 项数×高)估算 clamp，渲染后再按
// 实际尺寸修正一次，保证菜单完整落在视口内。
const MENU_WIDTH = 180;
const MENU_ITEM_HEIGHT = props.isMobile ? 44 : 36;

function positionMenu(): void {
  const rect = menu.value?.getBoundingClientRect();
  const height = rect?.height ?? MENU_ITEM_HEIGHT * 3 + 10;
  const left = Math.max(8, Math.min(props.x, window.innerWidth - MENU_WIDTH - 8));
  const top = Math.max(8, Math.min(props.y, window.innerHeight - height - 8));
  menuStyle.value = { left: `${left}px`, top: `${top}px`, width: `${MENU_WIDTH}px` };
}

function run(action: (entry: LocalClipboardEntry) => void): (entry: LocalClipboardEntry) => void {
  return (entry) => {
    emit("close");
    action(entry);
  };
}

const onActivate = run((entry) => emit("activate", entry));
const onSave = run((entry) => emit("save", entry));
const onRemove = run((entry) => emit("remove", entry));

function handleMenuKeydown(event: KeyboardEvent): void {
  event.stopPropagation();
  const items = Array.from(menu.value?.querySelectorAll<HTMLButtonElement>("button") ?? []);
  const currentIndex = items.indexOf(document.activeElement as HTMLButtonElement);
  if (event.key === "Escape") {
    event.preventDefault();
    emit("close");
    return;
  }
  if (event.key === "Tab") {
    emit("close");
    return;
  }
  const targetIndex = event.key === "ArrowDown"
    ? Math.min(items.length - 1, currentIndex + 1)
    : event.key === "ArrowUp"
      ? Math.max(0, currentIndex - 1)
      : event.key === "Home"
        ? 0
        : event.key === "End"
          ? items.length - 1
          : -1;
  if (targetIndex < 0) return;
  event.preventDefault();
  items[targetIndex]?.focus();
}

function handlePointerDown(event: PointerEvent): void {
  if (menu.value?.contains(event.target as Node)) return;
  emit("close");
}

function handleViewportChange(): void {
  positionMenu();
}

onMounted(() => {
  void nextTick(() => {
    positionMenu();
    menu.value?.querySelector<HTMLButtonElement>("button")?.focus();
  });
  document.addEventListener("pointerdown", handlePointerDown);
  window.addEventListener("resize", handleViewportChange);
  window.addEventListener("scroll", handleViewportChange, true);
});

onBeforeUnmount(() => {
  document.removeEventListener("pointerdown", handlePointerDown);
  window.removeEventListener("resize", handleViewportChange);
  window.removeEventListener("scroll", handleViewportChange, true);
});
</script>

<template>
  <Teleport to="body">
    <div
      ref="menu"
      class="entry-context-menu"
      role="menu"
      :aria-label="`条目操作：${entry.content}`"
      :style="menuStyle"
      @keydown="handleMenuKeydown"
    >
      <button type="button" role="menuitem" :disabled="activating" @click="onActivate(entry)">
        <LoaderCircle v-if="activating" :size="15" class="spin" aria-hidden="true" />
        <Copy v-else :size="15" aria-hidden="true" />
        <span>复制</span>
      </button>
      <button
        v-if="canSaveEntry(entry)"
        type="button"
        role="menuitem"
        :disabled="saving"
        @click="onSave(entry)"
      >
        <LoaderCircle v-if="saving" :size="15" class="spin" aria-hidden="true" />
        <Download v-else :size="15" aria-hidden="true" />
        <span>{{ saveLabel }}</span>
      </button>
      <button type="button" role="menuitem" class="danger" @click="onRemove(entry)">
        <Trash2 :size="15" aria-hidden="true" />
        <span>删除</span>
      </button>
    </div>
  </Teleport>
</template>

<style scoped>
.entry-context-menu { position: fixed; z-index: 60; display: grid; gap: 2px; padding: 5px; background: #111c31; border: 1px solid rgba(148, 163, 184, 0.24); border-radius: 8px; box-shadow: 0 14px 36px rgba(2, 6, 23, 0.62); }
.entry-context-menu button { display: grid; grid-template-columns: 18px 1fr; align-items: center; min-height: 36px; padding: 0 9px; color: #cbd5e1; text-align: left; background: transparent; border: 0; border-radius: 5px; font-size: 12px; cursor: pointer; }
.entry-context-menu button:hover:not(:disabled), .entry-context-menu button:focus-visible:not(:disabled) { color: #f8fafc; background: rgba(96, 165, 250, 0.13); outline: 0; }
.entry-context-menu button:disabled { color: #64748b; cursor: default; }
.entry-context-menu button.danger { color: #fca5a5; }
.entry-context-menu button.danger:hover:not(:disabled), .entry-context-menu button.danger:focus-visible:not(:disabled) { color: #fecaca; background: rgba(248, 113, 113, 0.14); }
@media (max-width: 640px) {
  .entry-context-menu button { min-height: 44px; font-size: 14px; }
}
</style>
