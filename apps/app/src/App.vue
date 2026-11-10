<script setup lang="ts">
import RustSettingsDialog from "./features/settings/RustSettingsDialog.vue";
import SharedApp from "@cliproam/ui/app";
import HistoryView from "@cliproam/ui/history";
import { isNative, isQuickPaste } from "@cliproam/ui/platform";
import UpdaterDialog from "./features/updater/UpdaterDialog.vue";
import { onBeforeUnmount, onMounted } from "vue";
import { native } from "@cliproam/ui/native";
import { initializeBossKey } from "./features/settings/bossKey";
import { initializeStickyDesktop } from "./features/sticky/stickyWindows";
import { initializeIosShortcuts } from "./features/shortcuts/iosShortcuts";
import { initializeIosWidget } from "./features/sticky/iosWidget";
import { initializeAndroidWidget } from "./features/sticky/androidWidget";
import { initializeAndroidSms } from "./features/shortcuts/androidSms";

let disposeSticky: (() => void) | undefined;
let disposeIosShortcuts: (() => void) | undefined;
let disposeIosWidget: (() => void) | undefined;
let disposeAndroidWidget: (() => void) | undefined;
let disposeAndroidSms: (() => void) | undefined;
let disposed = false;
onBeforeUnmount(() => { disposed = true; disposeSticky?.(); disposeIosShortcuts?.(); disposeIosWidget?.(); disposeAndroidWidget?.(); disposeAndroidSms?.(); });

onMounted(async () => {
  try {
    const capabilities = await native.getCapabilities();
    if (capabilities.mobile) {
      const device = await native.getDeviceInfo();
      if (device.osType === "ios" && !disposed) {
        disposeIosShortcuts = initializeIosShortcuts(device.osVersion);
        disposeIosWidget = initializeIosWidget();
      } else if (device.osType === "android" && !disposed) {
        disposeAndroidWidget = initializeAndroidWidget();
        disposeAndroidSms = initializeAndroidSms();
      }
    }
    if (capabilities.globalShortcut) await initializeBossKey();
    if (isNative && !capabilities.mobile) {
      disposeSticky = await initializeStickyDesktop();
      if (disposed) disposeSticky();
    }
  } catch (error) {
    console.warn("桌面功能初始化失败", error);
  }
});
</script>

<template>
  <SharedApp>
    <template #quick-paste="{ bindings, register }">
      <HistoryView :ref="register" v-bind="bindings" quick-paste />
    </template>
    <template #settings="{ currentUsername }">
      <RustSettingsDialog :current-username="currentUsername" />
    </template>
    <template #native-overlays>
      <UpdaterDialog v-if="isNative && !isQuickPaste" />
    </template>
  </SharedApp>
</template>
