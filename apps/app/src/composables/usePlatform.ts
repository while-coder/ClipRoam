import { computed, ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { DESKTOP_CAPABILITIES } from "../utils/constants";
import type { PlatformCapabilities } from "../types";

/** Window identity and platform capabilities, shared by all window roots. */

export const runningInTauri = "__TAURI_INTERNALS__" in window;
export const isPasteWindow = runningInTauri && getCurrentWindow().label === "paste";
export const isToastWindow = runningInTauri && getCurrentWindow().label === "toast";
if (isToastWindow) document.documentElement.classList.add("toast-window-root");

const platformCapabilities = ref<PlatformCapabilities>(DESKTOP_CAPABILITIES);

export function usePlatform() {
  const isMobile = computed(() => platformCapabilities.value.mobile);
  return {
    platformCapabilities,
    isMobile,
    setPlatformCapabilities(capabilities: PlatformCapabilities): void {
      platformCapabilities.value = capabilities;
    },
  };
}
