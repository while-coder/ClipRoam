<script setup lang="ts">
import RustSettingsDialog from "./features/settings/RustSettingsDialog.vue";
import IosShortcutsCard from "./features/settings/IosShortcutsCard.vue";
import SharedApp from "@cliproam/ui/app";
import HistoryView from "@cliproam/ui/history";
import { isNative, isQuickPaste } from "@cliproam/ui/platform";
import UpdaterDialog from "./features/updater/UpdaterDialog.vue";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { invoke, addPluginListener } from "@tauri-apps/api/core";
import { native } from "@cliproam/ui/native";
import { initializeBossKey } from "./features/settings/bossKey";
import { initializeStickyDesktop } from "./features/sticky/stickyWindows";
import { iosShortcutsAvailable, initializeIosShortcuts } from "./features/shortcuts/iosShortcuts";
import { initializeIosWidget } from "./features/sticky/iosWidget";
import { initializeAndroidWidget } from "./features/sticky/androidWidget";
import { initializeAndroidSms } from "./features/shortcuts/androidSms";

const mobileSettingsSections = computed(() => iosShortcutsAvailable.value ? [
  { id: "ios-shortcuts", title: "快捷指令", description: "短信自动同步、文本与文件上传" },
] : []);

const sharedApp = ref<InstanceType<typeof SharedApp>>();
let disposeWidgetNavigation: (() => void) | undefined;
async function initializeWidgetNavigation(android: boolean): Promise<void> {
  const consume = async () => {
    try {
      const request = await invoke<{ id?: string }>("native_pending_widget_entry");
      if (!disposed && request.id) sharedApp.value?.openWidgetEntry(request.id);
    } catch (error) { console.warn("读取小组件跳转失败", error); }
  };
  const stop = android
    ? await addPluginListener("cliproam-share-receiver", "widget-open", consume).then((listener) => () => listener.unregister())
    : await native.listen("cliproam://widget-open", consume);
  if (disposed) { stop(); return; }
  disposeWidgetNavigation = stop;
  await consume();
}

let disposeSticky: (() => void) | undefined;
let disposeIosShortcuts: (() => void) | undefined;
let disposeIosWidget: (() => void) | undefined;
let disposeAndroidWidget: (() => void) | undefined;
let disposeAndroidSms: (() => void) | undefined;
let disposed = false;
onBeforeUnmount(() => { disposed = true; disposeSticky?.(); disposeIosShortcuts?.(); disposeIosWidget?.(); disposeAndroidWidget?.(); disposeAndroidSms?.(); disposeWidgetNavigation?.(); });

onMounted(async () => {
  try {
    const capabilities = await native.getCapabilities();
    if (capabilities.mobile) {
      const device = await native.getDeviceInfo();
      await initializeWidgetNavigation(device.osType === "android");
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
  <SharedApp ref="sharedApp" :mobile-settings-sections="mobileSettingsSections">
    <template #quick-paste="{ bindings, register }">
      <HistoryView :ref="register" v-bind="bindings" quick-paste />
    </template>
    <template #settings="{ currentUsername }">
      <RustSettingsDialog :current-username="currentUsername" />
    </template>
    <template #mobile-settings-content="{ currentUsername, section }">
      <IosShortcutsCard v-if="section === 'ios-shortcuts'" :current-username="currentUsername" />
    </template>
    <template #native-overlays>
      <UpdaterDialog v-if="isNative && !isQuickPaste" />
    </template>
  </SharedApp>
</template>
