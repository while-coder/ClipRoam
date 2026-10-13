import { nextTick } from "vue";
import { invoke } from "@tauri-apps/api/core";
import {
  cursorPosition,
  getCurrentWindow,
  LogicalSize,
  monitorFromPoint,
  PhysicalPosition,
  type PhysicalSize,
} from "@tauri-apps/api/window";
import { isMobile, isQuickPaste, isToastWindow } from "../../composables/usePlatform";
import { getAccountSession } from "../sync/accountSession";
import { focusSearch, focusSearchInput } from "../history/useHistorySync";
import { isNative } from "../../platform/runtime";

type MainWindowState = {
  size: PhysicalSize;
  position: PhysicalPosition;
  maximized: boolean;
  fullscreen: boolean;
};

let mainState: MainWindowState | undefined;
let quickPasteSize: PhysicalSize | undefined;
let transitionQueue: Promise<void> = Promise.resolve();

// Serialize tray/shortcut requests so quick geometry cannot replace main geometry.
function transition(action: () => Promise<void>): Promise<void> {
  const result = transitionQueue.then(action);
  transitionQueue = result.catch(() => undefined);
  return result;
}

function setQuickPasteView(enabled: boolean): void {
  isQuickPaste.value = enabled;
  document.documentElement.classList.toggle("paste-window-root", enabled);
}

async function setSkipTaskbar(skip: boolean): Promise<void> {
  // Tauri's taskbar visibility API is unsupported on macOS.
  if (!/Mac/i.test(navigator.platform)) await getCurrentWindow().setSkipTaskbar(skip);
}

async function restoreMainWindow(): Promise<void> {
  const window = getCurrentWindow();
  await invoke("native_set_quick_paste_mode", { enabled: false });
  await window.hide();
  if (isQuickPaste.value) quickPasteSize = await window.innerSize();
  setQuickPasteView(false);
  await window.setDecorations(true);
  await window.setAlwaysOnTop(false);
  await setSkipTaskbar(false);
  await window.setTitle("ClipRoam");
  await window.setMinSize(new LogicalSize(760, 600));
  if (mainState) {
    await window.setSize(mainState.size);
    await window.setPosition(mainState.position);
    if (mainState.maximized) await window.maximize();
    if (mainState.fullscreen) await window.setFullscreen(true);
    mainState = undefined;
  }
  await nextTick();
}

export function showMainWindow(): Promise<void> {
  if (!isNative || isMobile.value || isToastWindow) return Promise.resolve();
  return transition(async () => {
    const window = getCurrentWindow();
    if (isQuickPaste.value || mainState) await restoreMainWindow();
    await window.show();
    await window.unminimize();
    await window.setFocus();
    focusSearchInput();
  });
}

/** A webview reload loses TS geometry but must not leave login in quick mode. */
export function initializeWindowMode(): Promise<void> {
  if (!isNative || isMobile.value || isToastWindow) return Promise.resolve();
  return transition(async () => {
    const wasQuickPaste = await invoke<boolean>("native_set_quick_paste_mode", { enabled: false });
    if (!wasQuickPaste) return;
    await restoreMainWindow();
    const window = getCurrentWindow();
    await window.setSize(new LogicalSize(960, 700));
    await window.center();
    await window.show();
  });
}

export function showPasteWindow(): Promise<void> {
  if (!isNative || isMobile.value || isToastWindow) return Promise.resolve();
  return transition(async () => {
    const session = getAccountSession();
    if (!session) return;
    const window = getCurrentWindow();
    // Capture before hiding/focusing our window. Repeated shortcuts while
    // quick mode is visible preserve the original external target on macOS.
    await invoke("native_capture_paste_target");
    try {
      await invoke("native_set_quick_paste_mode", { enabled: false });
      await window.hide();
      if (!isQuickPaste.value) {
        const maximized = await window.isMaximized();
        const fullscreen = await window.isFullscreen();
        if (fullscreen) await window.setFullscreen(false);
        if (maximized) await window.unmaximize();
        mainState = {
          size: await window.innerSize(),
          position: await window.outerPosition(),
          maximized,
          fullscreen,
        };
        await window.setMinSize(new LogicalSize(480, 520));
        await window.setDecorations(false);
        await window.setAlwaysOnTop(true);
        await setSkipTaskbar(true);
        await window.setTitle("ClipRoam - 快速粘贴");
        await window.setSize(quickPasteSize ?? new LogicalSize(560, 640));
      }
      setQuickPasteView(true);
      await nextTick();
      try {
        const cursor = await cursorPosition();
        const monitor = await monitorFromPoint(cursor.x, cursor.y);
        if (monitor) {
          const size = await window.outerSize();
          const work = monitor.workArea;
          const minX = work.position.x + 8;
          const minY = work.position.y + 8;
          const maxX = Math.max(minX, work.position.x + work.size.width - size.width - 8);
          const maxY = Math.max(minY, work.position.y + work.size.height - size.height - 8);
          const below = cursor.y + 12;
          await window.setPosition(new PhysicalPosition(
            Math.round(Math.min(Math.max(cursor.x - size.width / 2, minX), maxX)),
            Math.round(Math.min(Math.max(below <= maxY ? below : cursor.y - size.height - 12, minY), maxY)),
          ));
        }
      } catch (error) {
        console.error("定位快捷粘贴窗口失败：", error);
      }
      session.signal.throwIfAborted();
      await window.show();
      await window.unminimize();
      await window.setFocus();
      // Enable native blur hiding only after the window has gained focus.
      await invoke("native_set_quick_paste_mode", { enabled: true });
      focusSearch();
    } catch (error) {
      await restoreMainWindow();
      await window.show();
      await window.setFocus();
      throw error;
    }
  });
}

export function hideWindow(): Promise<void> {
  if (!isNative || isMobile.value || isToastWindow) return Promise.resolve();
  return transition(() => getCurrentWindow().hide());
}
