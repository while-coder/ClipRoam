import { ref } from "vue";
import { emitTo } from "@tauri-apps/api/event";
import { errorMessage } from "../../utils/error";

const STORAGE_KEY = "cliproam.quickPasteShortcut";
export const DEFAULT_QUICK_PASTE_SHORTCUT = "CommandOrControl+Shift+V";

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
let registeredShortcut = "";
let shortcutApi: typeof import("@tauri-apps/plugin-global-shortcut") | undefined;
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
  shortcutApi ??= await import("@tauri-apps/plugin-global-shortcut");
  await shortcutApi.register(shortcut, (event) => {
    if (event.state === "Pressed") void emitTo("paste", "cliproam://show-paste");
  });
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
      shortcutApi ??= await import("@tauri-apps/plugin-global-shortcut");
      if (registeredShortcut === normalized && await shortcutApi.isRegistered(normalized)) {
        applied = true;
      } else {
        const previous = registeredShortcut;
        if (previous) await shortcutApi.unregister(previous).catch(() => undefined);
        try {
          await registerShortcut(normalized);
          registeredShortcut = normalized;
          applied = true;
        } catch (error) {
          // A repeated registration by this process can report an error on
          // Windows even though the shortcut remains active.
          if (await shortcutApi.isRegistered(normalized).catch(() => false)) {
            registeredShortcut = normalized;
            applied = true;
          } else {
            if (previous) {
              await registerShortcut(previous).then(
                () => { registeredShortcut = previous; },
                () => { registeredShortcut = ""; },
              );
            }
            console.error("注册快捷粘贴快捷键失败：", normalized, error);
          }
        }
      }

      if (!applied) {
        quickPasteShortcutStatus.value = {
          state: "error",
          message: "注册失败，可能与系统或其他程序冲突，请换一组",
        };
        return;
      }

      if (persist) {
        window.localStorage.setItem(STORAGE_KEY, normalized);
        savedShortcut = normalized;
        quickPasteShortcut.value = normalized;
      }
      quickPasteShortcutStatus.value = {
        state: "ok",
        message: `已生效：${displayShortcut(normalized)}`,
      };
    } catch (error) {
      quickPasteShortcutStatus.value = {
        state: "error",
        message: `快捷键初始化失败：${errorMessage(error)}`,
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
  if (shortcutApi && registeredShortcut) {
    await shortcutApi.unregister(registeredShortcut).catch(() => undefined);
  }
  registeredShortcut = "";
}
