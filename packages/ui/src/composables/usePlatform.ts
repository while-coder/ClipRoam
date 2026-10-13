import { computed, ref } from "vue";
import { DESKTOP_CAPABILITIES, BROWSER_CAPABILITIES } from "../utils/constants";
import { isNative } from "../platform/runtime";
import type { PlatformCapabilities } from "../types";

/** Window mode and platform capabilities, shared by the application. */

export const isQuickPaste = ref(false);

const platformCapabilities = ref<PlatformCapabilities>(isNative ? DESKTOP_CAPABILITIES : BROWSER_CAPABILITIES);
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
