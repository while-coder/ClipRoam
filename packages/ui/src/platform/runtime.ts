import { computed, ref } from "vue";
import { native } from "./native";
import { DESKTOP_CAPABILITIES, BROWSER_CAPABILITIES } from "../config";
import { toast } from "@qingfeng346/ui-kit";
import type { PlatformCapabilities, NotificationPayload, NotificationOptions } from "../types";
import packageInfo from "../../package.json";

export const isNative = native.kind !== "web";
export const previewSource = (path: string): Promise<string> => native.previewSource(path);

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

const fallbackAppVersion = packageInfo.version;
const appVersion = ref("");
let versionInitialized = false;

async function initAppVersion(): Promise<void> {
  if (versionInitialized) return;
  versionInitialized = true;

  try {
    appVersion.value = (await native.getAppVersion()) || fallbackAppVersion;
  } catch {
    appVersion.value = fallbackAppVersion;
  }
}

export function useAppVersion() {
  return {
    appVersion,
    initAppVersion,
  };
}

/** 原生通知与 UI 通知回退。 */
export function displayNotification(payload: NotificationPayload): void {
  toast.show(payload.tone, payload.message);
}

export function showNotification(options: NotificationOptions): void {
  const normalized = options.message.trim();
  if (!normalized) return;
  const payload: NotificationPayload = {
    message: normalized,
    title: options.title?.trim() || "ClipRoam",
    tone: options.tone ?? "info",
  };
  if (isMobile.value) {
    displayNotification(payload);
    return;
  }
  void native.showNotification(payload).catch(() => displayNotification(payload));
}

export type { NotificationOptions } from "../types";
