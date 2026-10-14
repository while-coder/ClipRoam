import { nextTick } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { native } from "@cliproam/ui/native";
import { isMobile, isQuickPaste, isNative } from "@cliproam/ui/platform";
import { getAccountSession } from "@cliproam/ui/account-session";
import { focusSearch, focusSearchInput } from "@cliproam/ui/history-search";
import {
  cursorPosition,
  getCurrentWindow,
  LogicalSize,
  monitorFromPoint,
  PhysicalPosition,
  type PhysicalSize,
} from "@tauri-apps/api/window";

type MainWindowState = {
  size: PhysicalSize;
  position: PhysicalPosition;
  maximized: boolean;
  fullscreen: boolean;
};

const QUICK_PASTE_WIDTH = 520;
let mainState: MainWindowState | undefined;
let quickPasteHeight: number | undefined;
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
  await native.setQuickPasteMode(false);
  await window.hide();
  if (isQuickPaste.value) {
    quickPasteHeight = (await window.innerSize()).toLogical(await window.scaleFactor()).height;
  }
  setQuickPasteView(false);
  await window.setDecorations(true);
  await window.setAlwaysOnTop(false);
  await setSkipTaskbar(false);
  await window.setTitle("ClipRoam");
  await window.setSizeConstraints({ minWidth: 760, minHeight: 600 });
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
  if (!isNative || isMobile.value || native.windowLabel === "toast") return Promise.resolve();
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
  if (!isNative || isMobile.value || native.windowLabel === "toast") return Promise.resolve();
  return transition(async () => {
    const wasQuickPaste = await native.setQuickPasteMode(false);
    if (!wasQuickPaste) return;
    await restoreMainWindow();
    const window = getCurrentWindow();
    await window.setSize(new LogicalSize(960, 700));
    await window.center();
    await window.show();
  });
}

export function showPasteWindow(): Promise<void> {
  if (!isNative || isMobile.value || native.windowLabel === "toast") return Promise.resolve();
  return transition(async () => {
    const session = getAccountSession();
    if (!session) return;
    const window = getCurrentWindow();
    // Capture before hiding/focusing our window. Repeated shortcuts while
    // quick mode is visible preserve the original external target on macOS.
    await native.capturePasteTarget();
    try {
      await native.setQuickPasteMode(false);
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
        await window.setSizeConstraints({ minWidth: QUICK_PASTE_WIDTH, maxWidth: QUICK_PASTE_WIDTH, minHeight: 520 });
        await window.setDecorations(false);
        await window.setAlwaysOnTop(true);
        await setSkipTaskbar(true);
        await window.setTitle("ClipRoam - 快速粘贴");
        await window.setSize(new LogicalSize(QUICK_PASTE_WIDTH, quickPasteHeight ?? 640));
      }
      setQuickPasteView(true);
      await nextTick();
      // macOS 在原生显示命令中用 AppKit 逻辑点定位，不混用 Retina 像素坐标。
      if (!/Mac/i.test(navigator.platform)) {
        try {
          const cursor = await cursorPosition();
          const monitor = await monitorFromPoint(cursor.x, cursor.y);
          if (monitor) {
            const size = await window.outerSize();
            const work = monitor.workArea;
            const margin = 8 * monitor.scaleFactor;
            const gap = 12 * monitor.scaleFactor;
            const minX = work.position.x + margin;
            const minY = work.position.y + margin;
            const maxX = Math.max(minX, work.position.x + work.size.width - size.width - margin);
            const maxY = Math.max(minY, work.position.y + work.size.height - size.height - margin);
            const right = cursor.x + gap;
            const below = cursor.y + gap;
            await window.setPosition(new PhysicalPosition(
              Math.round(Math.min(Math.max(right <= maxX ? right : cursor.x - size.width - gap, minX), maxX)),
              Math.round(Math.min(Math.max(below <= maxY ? below : cursor.y - size.height - gap, minY), maxY)),
            ));
          }
        } catch (error) {
          console.error("定位快捷粘贴窗口失败：", error);
        }
      }
      session.signal.throwIfAborted();
      await invoke("native_focus_quick_paste_window");
      // macOS 激活应用是异步的；尚未获焦就开启失焦隐藏会把窗口立即收起。
      if (/Mac/i.test(navigator.platform)) {
        for (let attempt = 0; ; attempt++) {
          session.signal.throwIfAborted();
          if (await window.isFocused()) break;
          if (attempt >= 20) throw new Error("快捷粘贴窗口未能获得焦点，请重试");
          await new Promise<void>((resolve) => setTimeout(resolve, 25));
        }
      }
      // Enable native blur hiding only after the window has gained focus.
      await native.setQuickPasteMode(true);
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
  if (!isNative || isMobile.value || native.windowLabel === "toast") return Promise.resolve();
  return transition(() => getCurrentWindow().hide());
}
