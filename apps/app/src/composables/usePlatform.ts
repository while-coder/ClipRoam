import { computed, ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { DESKTOP_CAPABILITIES } from "../utils/constants";
import type { PlatformCapabilities } from "../types";

/** Window identity and platform capabilities, shared by all window roots. */

export const isPasteWindow = getCurrentWindow().label === "paste";
export const isToastWindow = getCurrentWindow().label === "toast";
if (isToastWindow) document.documentElement.classList.add("toast-window-root");

const platformCapabilities = ref<PlatformCapabilities>(DESKTOP_CAPABILITIES);
/** 模块级单例：后台模块（引擎、条目动作）不经过 usePlatform() 也要读移动端分支。 */
export const isMobile = computed(() => platformCapabilities.value.mobile);

export function usePlatform() {
  return {
    platformCapabilities,
    isMobile,
    setPlatformCapabilities(capabilities: PlatformCapabilities): void {
      platformCapabilities.value = capabilities;
    },
  };
}
