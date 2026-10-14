import { nextTick } from "vue";
import { native } from "@cliproam/ui/native";
import { isMobile, isQuickPaste, isNative } from "@cliproam/ui/platform";
import { getAccountSession } from "@cliproam/ui/account-session";
import { focusSearch, focusSearchInput } from "@cliproam/ui/history-search";
import { getCurrentWindow } from "@tauri-apps/api/window";

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
    await native.setWindowMode({ mode: "main", title: "ClipRoam", visible: false });
    setQuickPasteView(false);
    await nextTick();
  }
  await native.setWindowMode({ mode: "main", title: "ClipRoam" });
}

export function showMainWindow(): Promise<void> {
  if (!supportsWindowMode()) return Promise.resolve();
  return transition(async () => {
    await restoreMainWindow();
    focusSearchInput();
  });
}

/** Native geometry survives a webview reload and restores login to main mode. */
export function initializeWindowMode(): Promise<void> {
  if (!supportsWindowMode()) return Promise.resolve();
  return transition(restoreMainWindow);
}

export function showPasteWindow(): Promise<void> {
  if (!supportsWindowMode()) return Promise.resolve();
  return transition(async () => {
    const session = getAccountSession();
    if (!session) return;
    // Capture before hiding/focusing our window; repeated shortcuts preserve
    // the original external paste target on macOS.
    await native.capturePasteTarget();
    try {
      await native.setWindowMode({ mode: "quickPaste", title: "ClipRoam - 快速粘贴", visible: false });
      setQuickPasteView(true);
      await nextTick();
      session.signal.throwIfAborted();
      await native.setWindowMode({ mode: "quickPaste", title: "ClipRoam - 快速粘贴", position: { kind: "cursor" } });
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
