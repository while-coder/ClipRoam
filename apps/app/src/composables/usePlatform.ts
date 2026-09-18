import { computed, ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { DESKTOP_CAPABILITIES } from "../utils/constants";
import type { PlatformCapabilities } from "../types";

/** Window identity and platform capabilities, shared by all window roots. */

export const runningInTauri = "__TAURI_INTERNALS__" in window;
// [dev] 浏览器预览可用 ?paste=1 强制快速粘贴窗口布局，便于桌面调试。
const forcePaste = new URLSearchParams(window.location.search).has("paste");
export const isPasteWindow = forcePaste
  || (runningInTauri && getCurrentWindow().label === "paste");
export const isToastWindow = runningInTauri && getCurrentWindow().label === "toast";
if (isToastWindow) document.documentElement.classList.add("toast-window-root");

// [dev] 浏览器预览可用 ?mobile=1 强制移动端布局，便于桌面调试。
const forceMobile = new URLSearchParams(window.location.search).has("mobile");
const platformCapabilities = ref<PlatformCapabilities>(
  forceMobile ? { ...DESKTOP_CAPABILITIES, mobile: true } : DESKTOP_CAPABILITIES,
);
/** 模块级单例：后台模块（引擎、条目动作）不经过 usePlatform() 也要读移动端分支。 */
export const isMobile = computed(() => platformCapabilities.value.mobile);

export function usePlatform() {
  return {
    platformCapabilities,
    isMobile,
    setPlatformCapabilities(capabilities: PlatformCapabilities): void {
      if (forceMobile) return;
      platformCapabilities.value = capabilities;
    },
  };
}
