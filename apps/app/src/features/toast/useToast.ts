import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { isToastWindow, runningInTauri } from "../../composables/usePlatform";
import type { ToastPayload, ToastTone } from "../../types";

/** 模块级单例（对齐 usePlatform 风格）：所有窗口内组件共享同一个 toast 状态。 */
const toastPayload = ref<ToastPayload>();

let toastTimer: number | undefined;
let toastWindowHideTimer: number | undefined;

function displayToast(payload: ToastPayload): void {
  if (toastTimer !== undefined) window.clearTimeout(toastTimer);
  if (toastWindowHideTimer !== undefined) window.clearTimeout(toastWindowHideTimer);
  toastPayload.value = payload;
  toastTimer = window.setTimeout(() => {
    toastPayload.value = undefined;
    toastTimer = undefined;
    if (isToastWindow) {
      toastWindowHideTimer = window.setTimeout(() => {
        void invoke("hide_toast");
        toastWindowHideTimer = undefined;
      }, 180);
    }
  }, payload.tone === "error" ? 5_000 : 3_200);
}

function showToast(message: string, tone: ToastTone = "info"): void {
  const normalized = message.trim();
  if (!normalized) return;
  const payload = { message: normalized, tone };
  if (!runningInTauri) {
    displayToast(payload);
    return;
  }
  void invoke("show_toast", payload).catch(() => displayToast(payload));
}

/** 清理 toast 相关 timer（组件卸载时调用）。 */
function disposeToast(): void {
  if (toastTimer !== undefined) window.clearTimeout(toastTimer);
  if (toastWindowHideTimer !== undefined) window.clearTimeout(toastWindowHideTimer);
}

/** toast 专用窗口（tray 旁的通知窗口）只监听 toast 事件，不初始化其他服务。 */
function startToastWindowListener(): Promise<UnlistenFn> {
  return listen<ToastPayload>("cliproam://toast", ({ payload }) => {
    displayToast(payload);
  });
}

export { toastPayload, showToast, disposeToast, startToastWindowListener };
