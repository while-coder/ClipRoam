import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { errorMessage } from "@cliproam/ui/errors";

const STORAGE_KEY = "cliproam.quickPasteShortcut";
const DEFAULT_QUICK_PASTE_SHORTCUT = "CommandOrControl+Shift+V";

type ShortcutStatus = {
  state: "idle" | "ok" | "error";
  message: string;
};

function readSavedShortcut(): string {
  try {
    return window.localStorage.getItem(STORAGE_KEY)?.trim() || DEFAULT_QUICK_PASTE_SHORTCUT;
  } catch {
    return DEFAULT_QUICK_PASTE_SHORTCUT;
  }
}

let savedShortcut = readSavedShortcut();
let refreshQueue: Promise<void> = Promise.resolve();

export const quickPasteShortcut = ref(savedShortcut);
export const quickPasteShortcutStatus = ref<ShortcutStatus>({ state: "idle", message: "" });
export const quickPasteShortcutRefreshing = ref(false);

export function displayShortcut(shortcut: string): string {
  const commandKey = /Mac|iPhone|iPad/i.test(navigator.platform) ? "⌘" : "Ctrl";
  return shortcut.replace("CommandOrControl", commandKey);
}

export function resetQuickPasteShortcutDraft(): void {
  quickPasteShortcut.value = savedShortcut;
}

async function registerShortcut(shortcut: string): Promise<void> {
  // 唤起由原生全局处理器转发，不依赖后台或重载前的 webview 回调。
  await invoke("native_register_quick_paste_shortcut", { shortcut });
}

async function applyShortcut(shortcut: string, persist: boolean): Promise<boolean> {
  const normalized = shortcut.trim();
  quickPasteShortcutRefreshing.value = true;
  quickPasteShortcutStatus.value = { state: "idle", message: "正在应用快捷键…" };

  let applied = false;
  const refresh = async () => {
    if (!normalized) {
      quickPasteShortcutStatus.value = { state: "error", message: "请先录制快捷键" };
      return;
    }

    try {
      await registerShortcut(normalized);

      if (persist) {
        window.localStorage.setItem(STORAGE_KEY, normalized);
        savedShortcut = normalized;
        quickPasteShortcut.value = normalized;
      }
      quickPasteShortcutStatus.value = {
        state: "ok",
        message: `已生效：${displayShortcut(normalized)}`,
      };
      applied = true;
    } catch (error) {
      quickPasteShortcutStatus.value = {
        state: "error",
        message: `快捷键注册失败，请检查是否冲突：${errorMessage(error)}`,
      };
    }
  };

  const queued = refreshQueue.then(refresh, refresh);
  refreshQueue = queued.then(() => undefined, () => undefined);
  await queued;
  quickPasteShortcutRefreshing.value = false;
  return applied;
}

export function initializeQuickPasteShortcut(): Promise<boolean> {
  return applyShortcut(savedShortcut, false);
}

export function saveQuickPasteShortcut(): Promise<boolean> {
  return applyShortcut(quickPasteShortcut.value, true);
}

export async function disposeQuickPasteShortcut(): Promise<void> {
  await refreshQueue;
  await invoke("native_register_quick_paste_shortcut", { shortcut: null }).catch(() => undefined);
}
