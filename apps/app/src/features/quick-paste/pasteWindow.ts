import { invoke } from "@tauri-apps/api/core";
import {
  cursorPosition,
  getCurrentWindow,
  monitorFromPoint,
  PhysicalPosition,
  type Monitor,
} from "@tauri-apps/api/window";
import { isMobile, isPasteWindow, runningInTauri } from "../../composables/usePlatform";
import { requestProxyDevices } from "../sync/bridge";
import { focusSearch } from "../clipboard-history/useHistorySync";

// Mirrors the former Rust-side paste positioning: center the window below the
// cursor and clamp it inside the monitor's work area. All values are physical
// pixels.
function calculatePasteWindowPosition(
  cursorX: number,
  cursorY: number,
  workX: number,
  workY: number,
  workWidth: number,
  workHeight: number,
  windowWidth: number,
  windowHeight: number,
): { x: number; y: number } {
  const CURSOR_GAP = 12;
  const SCREEN_MARGIN = 8;
  const minX = workX + SCREEN_MARGIN;
  const minY = workY + SCREEN_MARGIN;
  const maxX = Math.max(workX + workWidth - windowWidth - SCREEN_MARGIN, minX);
  const maxY = Math.max(workY + workHeight - windowHeight - SCREEN_MARGIN, minY);
  const x = Math.min(Math.max(cursorX - Math.floor(windowWidth / 2), minX), maxX);
  const belowCursor = cursorY + CURSOR_GAP;
  const preferredY = belowCursor <= maxY ? belowCursor : cursorY - windowHeight - CURSOR_GAP;
  return { x, y: Math.min(Math.max(preferredY, minY), maxY) };
}

export async function showPasteWindow(): Promise<void> {
  if (!isPasteWindow || !runningInTauri) return;
  // 每次弹出时向主窗口要一次设备名，兜住错过广播的启动竞态；失败静默。
  requestProxyDevices();
  // 必须在窗口获得焦点前记录前台应用；macOS 合成粘贴后靠它恢复焦点。
  await invoke("capture_paste_target").catch(() => undefined);
  // 主窗口可见时会参与焦点竞争：应用被点击激活时主窗口作为 key 窗口
  // 会吞掉对粘贴窗口的第一次点击。快速粘贴期间把主窗口收起，单击才能直达。
  await invoke("hide_main").catch(() => undefined);
  const pasteWindow = getCurrentWindow();
  try {
    const cursor = await cursorPosition();
    const monitor: Monitor | null = await monitorFromPoint(cursor.x, cursor.y);
    if (monitor) {
      const windowSize = await pasteWindow.outerSize();
      const workArea = monitor.workArea;
      const position = calculatePasteWindowPosition(
        Math.round(cursor.x),
        Math.round(cursor.y),
        workArea.position.x,
        workArea.position.y,
        workArea.size.width,
        workArea.size.height,
        windowSize.width,
        windowSize.height,
      );
      await pasteWindow.setPosition(new PhysicalPosition(position.x, position.y));
    }
  } catch (error) {
    // The window still opens even if positioning is unavailable.
    console.error("定位快捷粘贴窗口失败：", error);
  }
  await pasteWindow.show();
  await pasteWindow.unminimize();
  await pasteWindow.setFocus();
  focusSearch();
}

export async function hideWindow(): Promise<void> {
  if (runningInTauri && !isMobile.value) await invoke(isPasteWindow ? "hide_paste" : "hide_main");
}
