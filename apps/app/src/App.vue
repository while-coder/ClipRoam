<script setup lang="ts">
import RustSettingsDialog from "./features/settings/RustSettingsDialog.vue";
import SharedApp from "@cliproam/ui/app";
import HistoryView from "@cliproam/ui/history";
import { isNative, isQuickPaste } from "@cliproam/ui/platform";
import UpdaterDialog from "./features/updater/UpdaterDialog.vue";
import { onMounted } from "vue";
import { native } from "@cliproam/ui/native";
import { initializeBossKey } from "./features/settings/bossKey";

onMounted(async () => {
  if (isNative) {
    try {
      if ((await native.getCapabilities()).globalShortcut) await initializeBossKey();
    } catch (error) {
      console.warn("老板键初始化失败", error);
    }
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
