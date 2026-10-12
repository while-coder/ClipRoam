<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { FilePlus, FolderPlus, Menu, Settings2 } from "lucide-vue-next";
import { isMobile } from "../composables/usePlatform";
import { openSettings } from "../features/settings/useSettings";
import { connectionStatus } from "../features/sync/syncEngine";
import { uploadingFiles, captureFilesFromPicker } from "../features/pending-upload/useClipboardCapture";

const root = ref<HTMLElement>();
const trigger = ref<HTMLButtonElement>();
const menu = ref<HTMLElement>();
const open = ref(false);

function items(): HTMLButtonElement[] {
  return Array.from(menu.value?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
}

function close(restoreFocus = false): void {
  open.value = false;
  if (restoreFocus) trigger.value?.focus();
}

async function toggle(): Promise<void> {
  if (open.value) { close(true); return; }
  open.value = true;
  await nextTick();
  items()[0]?.focus();
}

function run(action: () => unknown): void {
  close(true);
  action();
}

function handleKeydown(event: KeyboardEvent): void {
  if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    close(true);
  } else if (event.key === "Tab") {
    close(true);
  } else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
    event.preventDefault();
    event.stopPropagation();
    const buttons = items();
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1
      : (index + (event.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length;
    buttons[next]?.focus();
  }
}

function handlePointerDown(event: PointerEvent): void {
  if (open.value && !root.value?.contains(event.target as Node)) close();
}

onMounted(() => document.addEventListener("pointerdown", handlePointerDown));
onBeforeUnmount(() => document.removeEventListener("pointerdown", handlePointerDown));
</script>

<template>
  <div ref="root" class="workspace-menu" @keydown.stop>
    <button ref="trigger" class="workspace-menu-trigger" type="button" aria-label="工作区菜单"
      aria-haspopup="menu" :aria-expanded="open" @click="toggle" @keydown.down.prevent="!open && toggle()">
      <span v-if="uploadingFiles" class="s-spinner" aria-label="正在导入内容" />
      <Menu v-else :size="20" aria-hidden="true" />
    </button>
    <div v-if="open" ref="menu" class="workspace-menu-popover" role="menu" aria-label="工作区操作" @keydown="handleKeydown">
      <span class="workspace-menu-status" :class="connectionStatus.tone" :title="connectionStatus.title" role="status">{{ connectionStatus.label }}</span>
      <button type="button" role="menuitem" :disabled="uploadingFiles" @click="run(() => captureFilesFromPicker('file'))">
        <FilePlus :size="18" aria-hidden="true" /><span>上传文件</span>
      </button>
      <button type="button" role="menuitem" :disabled="uploadingFiles || isMobile" :title="isMobile ? '移动端暂不支持上传文件夹' : undefined" @click="run(() => captureFilesFromPicker('folder'))">
        <FolderPlus :size="18" aria-hidden="true" /><span>上传文件夹<small v-if="isMobile">当前设备不支持</small></span>
      </button>
      <button type="button" role="menuitem" @click="run(openSettings)">
        <Settings2 :size="18" aria-hidden="true" /><span>设置</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.workspace-menu { position: relative; flex-shrink: 0; }
.workspace-menu-trigger {
  display: grid; place-items: center; width: 44px; height: 44px; padding: 0;
  color: var(--color-foreground); background: transparent; border: 1px solid var(--border);
  border-radius: 8px; cursor: pointer; touch-action: manipulation;
}
.workspace-menu-popover {
  position: absolute; z-index: 60; top: calc(100% + 8px); right: 0;
  display: grid; width: min(240px, calc(100vw - 32px)); max-height: calc(100dvh - 100px); padding: 6px;
  overflow-y: auto; overscroll-behavior: contain;
  color: var(--color-foreground); background: var(--color-surface); border: 1px solid var(--border);
  border-radius: 10px; box-shadow: 0 12px 32px var(--sui-mask);
}
.workspace-menu-popover > button {
  display: flex; align-items: center; gap: 12px; min-height: 48px; padding: 8px 12px;
  text-align: left; background: transparent; border: 0; border-radius: 6px;
  font-size: 14px; cursor: pointer; touch-action: manipulation;
}
.workspace-menu-popover > button > svg { flex-shrink: 0; }
.workspace-menu-popover small { display: block; margin-top: 3px; font-size: 12px; }
.workspace-menu-popover > button:disabled { opacity: 0.5; cursor: default; }
.workspace-menu-trigger:hover, .workspace-menu-popover > button:hover:not(:disabled) { background: var(--sui-bg-hover); }
.workspace-menu-trigger:active, .workspace-menu-popover > button:active:not(:disabled) { background: var(--sui-bg-active); }
.workspace-menu-status { padding: 8px 12px; color: var(--color-secondary-text); font-size: 12px; }
.workspace-menu-status.online { color: var(--color-accent); }
</style>
