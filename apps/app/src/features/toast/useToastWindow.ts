import { ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { native, type Unlisten, type ToastPayload } from "@cliproam/ui/native";

/** 模块级单例（对齐 usePlatform 风格）：所有窗口内组件共享同一个 toast 状态。 */
const toastPayload = ref<ToastPayload>();

let toastTimer: number | undefined;
let toastWindowHideTimer: number | undefined;

function clearTimers(): void {
  if (toastTimer !== undefined) window.clearTimeout(toastTimer);
  if (toastWindowHideTimer !== undefined) window.clearTimeout(toastWindowHideTimer);
}

/** toast 消失后延迟隐藏通知窗口，让收尾动画播完。 */
function scheduleToastWindowHide(): void {
  toastWindowHideTimer = window.setTimeout(() => {
    void getCurrentWindow().hide().catch(() => {});
    toastWindowHideTimer = undefined;
  }, 180);
}

function displayToast(payload: ToastPayload): void {
  clearTimers();
  toastPayload.value = payload;
  toastTimer = window.setTimeout(() => {
    toastPayload.value = undefined;
    toastTimer = undefined;
    scheduleToastWindowHide();
  }, payload.tone === "error" ? 5_000 : 3_200);
}

/** 立即关闭 toast（托盘通知的关闭按钮）。 */
function hideToastNow(): void {
  clearTimers();
  toastPayload.value = undefined;
  toastTimer = undefined;
  scheduleToastWindowHide();
}

/** 清理 toast 相关 timer（组件卸载时调用）。 */
function disposeToast(): void {
  clearTimers();
}

/** toast 专用窗口（tray 旁的通知窗口）只监听 toast 事件，不初始化其他服务。 */
function startToastWindowListener(): Promise<Unlisten> {
  return native.listenToast(displayToast);
}

export { toastPayload, hideToastNow, disposeToast, startToastWindowListener };
