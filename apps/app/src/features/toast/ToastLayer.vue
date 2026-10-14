<script setup lang="ts">
import { X } from "lucide-vue-next";
import { hideToastNow, toastPayload } from "./useToastWindow";
</script>

<template>
  <!-- 桌面全局通知使用独立窗口，不抢主界面或外部应用的焦点。 -->
  <Transition name="toast">
    <aside
      v-if="toastPayload"
      class="toast-layer tray-toast"
      :class="`toast-${toastPayload.tone}`"
      :role="toastPayload.tone === 'error' ? 'alert' : 'status'"
      :aria-live="toastPayload.tone === 'error' ? 'assertive' : 'polite'"
      aria-atomic="true"
    >
      <span class="toast-card">
        <img class="toast-app-icon" src="/cliproam-icon.png" alt="" aria-hidden="true" />
        <span class="toast-body">
          <span class="toast-title">{{ toastPayload.title }}</span>
          <span class="toast-message">{{ toastPayload.message }}</span>
        </span>
        <button class="toast-close" type="button" aria-label="关闭通知" @click="hideToastNow">
          <X :size="14" aria-hidden="true" />
        </button>
      </span>
    </aside>
  </Transition>
</template>
