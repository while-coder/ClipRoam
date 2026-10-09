<script setup lang="ts">
import { X } from "lucide-vue-next";
import { isToastWindow } from "../../composables/usePlatform";
import { hideToastNow, toastPayload } from "./useToast";
</script>

<template>
  <!-- 收缩为托盘通知窗口专用；主窗口 toast 已改走 ui-kit（useToast displayToast）。 -->
  <Transition name="toast">
    <aside
      v-if="isToastWindow && toastPayload"
      class="toast-layer tray-toast"
      :class="`toast-${toastPayload.tone}`"
      :role="toastPayload.tone === 'error' ? 'alert' : 'status'"
      :aria-live="toastPayload.tone === 'error' ? 'assertive' : 'polite'"
      aria-atomic="true"
    >
      <span class="toast-card">
        <img class="toast-app-icon" src="/cliproam-icon.png" alt="" aria-hidden="true" />
        <span class="toast-body">
          <span class="toast-title">ClipRoam</span>
          <span class="toast-message">{{ toastPayload.message }}</span>
        </span>
        <button class="toast-close" type="button" aria-label="关闭通知" @click="hideToastNow">
          <X :size="14" aria-hidden="true" />
        </button>
      </span>
    </aside>
  </Transition>
</template>