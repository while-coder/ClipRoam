<script setup lang="ts">
import SharedApp from "@cliproam/ui/app";
import HistoryView from "@cliproam/ui/history";
import { isNative, isQuickPaste, isToastWindow } from "@cliproam/ui/runtime";
import { UpdaterDialog } from "@while-coder/tauri-updater-vue";
</script>

<template>
  <SharedApp>
    <template #quick-paste="{ bindings, register }">
      <HistoryView :ref="register" v-bind="bindings" quick-paste />
    </template>
    <template #native-overlays>
      <UpdaterDialog v-if="isNative && !isQuickPaste && !isToastWindow" locale="zh-CN" />
    </template>
  </SharedApp>
</template>
