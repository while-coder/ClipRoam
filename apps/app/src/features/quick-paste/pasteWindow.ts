import { nextTick } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { native } from "@cliproam/ui/native";
import { isMobile, isQuickPaste, isNative } from "@cliproam/ui/platform";
import { getAccountSession } from "@cliproam/ui/account-session";
import { focusSearch, focusSearchInput } from "@cliproam/ui/history-search";
import { getCurrentWindow } from "@tauri-apps/api/window";

type WindowModeOptions = {
  mode: "main" | "quickPaste";
  title?: string;
  /** Omit to preserve position; main mode restores its saved geometry. */
  position?: { kind: "cursor" } | { kind: "center" } | { kind: "coordinates"; x: number; y: number };
  /** Defaults to true. Use false to prepare the window before rendering its view. */
  visible?: boolean;
};

function setWindowMode(options: WindowModeOptions): Promise<void> {
  return invoke("native_set_window_mode", { ...options });
}

let transitionQueue: Promise<void> = Promise.resolve();

// Serialize tray/shortcut requests so window mode transitions cannot overlap.
function transition(action: () => Promise<void>): Promise<void> {
  const result = transitionQueue.then(action);
  transitionQueue = result.catch(() => undefined);
  return result;
}

function setQuickPasteView(enabled: boolean): void {
  isQuickPaste.value = enabled;
  document.documentElement.classList.toggle("paste-window-root", enabled);
}

function supportsWindowMode(): boolean {
  return isNative && !isMobile.value && native.windowLabel !== "toast";
}

async function restoreMainWindow(): Promise<void> {
  if (isQuickPaste.value) {
    await setWindowMode({ mode: "main", title: "随心贴", visible: false });
    setQuickPasteView(false);
    await nextTick();
  }
  await setWindowMode({ mode: "main", title: "随心贴" });
}

export function showMainWindow(options: { focusSearch?: boolean } = {}): Promise<void> {
  if (!supportsWindowMode()) return Promise.resolve();
  return transition(async () => {
    await restoreMainWindow();
    if (options.focusSearch !== false) focusSearchInput();
  });
}

export function showPasteWindow(): Promise<void> {
  if (!supportsWindowMode()) return Promise.resolve();
  return transition(async () => {
    const session = getAccountSession();
    if (!session) return;
    // Capture before hiding/focusing our window; repeated shortcuts preserve
    // the original external paste target on macOS.
    await invoke<void>("native_capture_paste_target");
    try {
      await setWindowMode({ mode: "quickPaste", title: "随心贴 - 快速粘贴", visible: false });
      setQuickPasteView(true);
      await nextTick();
      session.signal.throwIfAborted();
      await setWindowMode({ mode: "quickPaste", title: "随心贴 - 快速粘贴", position: { kind: "cursor" } });
      session.signal.throwIfAborted();
      focusSearch();
    } catch (error) {
      await restoreMainWindow();
      throw error;
    }
  });
}

export function hideWindow(): Promise<void> {
  if (!supportsWindowMode()) return Promise.resolve();
  return transition(() => getCurrentWindow().hide());
}
