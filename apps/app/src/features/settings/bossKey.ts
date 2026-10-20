import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { errorMessage } from "@cliproam/ui/errors";
import { displayShortcut } from "../quick-paste/quickPasteShortcut";

const STORAGE_KEY = "cliproam.bossKey";
const DEFAULT_SHORTCUT = "Alt+Shift+H";

function readSettings(): { enabled: boolean; shortcut: string } {
  try {
    const saved = JSON.parse(window.localStorage.getItem(STORAGE_KEY) ?? "null");
    return {
      enabled: typeof saved?.enabled === "boolean" ? saved.enabled : true,
      shortcut: typeof saved?.shortcut === "string" && saved.shortcut.trim() ? saved.shortcut.trim() : DEFAULT_SHORTCUT,
    };
  } catch {
    return { enabled: true, shortcut: DEFAULT_SHORTCUT };
  }
}

let saved = readSettings();
let queue: Promise<unknown> = Promise.resolve();
export const bossKeyEnabled = ref(saved.enabled);
export const bossKeyShortcut = ref(saved.shortcut);
export const bossKeyRefreshing = ref(false);
export const bossKeyStatus = ref<{ state: "idle" | "ok" | "error"; message: string }>({ state: "idle", message: "" });

export function resetBossKeyDraft(): void {
  bossKeyEnabled.value = saved.enabled;
  bossKeyShortcut.value = saved.shortcut;
}

function apply(settings: typeof saved, persist: boolean): Promise<boolean> {
  const run = async () => {
    bossKeyRefreshing.value = true;
    bossKeyStatus.value = { state: "idle", message: "正在应用老板键…" };
    try {
      if (settings.enabled && !settings.shortcut) throw new Error("请先录制快捷键");
      await invoke("native_set_boss_key", { shortcut: settings.enabled ? settings.shortcut : null });
      if (persist) {
        window.localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
        saved = settings;
      }
      bossKeyStatus.value = {
        state: "ok",
        message: settings.enabled ? `已生效：${displayShortcut(settings.shortcut)}` : "老板键已关闭",
      };
      return true;
    } catch (error) {
      bossKeyStatus.value = { state: "error", message: `老板键设置失败：${errorMessage(error)}` };
      return false;
    } finally {
      bossKeyRefreshing.value = false;
    }
  };
  const result = queue.then(run, run);
  queue = result;
  return result;
}

export function initializeBossKey(): Promise<boolean> {
  return apply(saved, false);
}

export function saveBossKey(): Promise<boolean> {
  return apply({ enabled: bossKeyEnabled.value, shortcut: bossKeyShortcut.value.trim() }, true);
}
