<script setup lang="ts">
import { CircleAlert, CircleCheck, Info, X } from "lucide-vue-next";
import { isToastWindow } from "../../composables/usePlatform";
import { hideToastNow, toastPayload } from "./useToast";
</script>

<template>
  <Transition name="toast">
    <aside
      v-if="toastPayload"
      class="toast-layer"
      :class="[`toast-${toastPayload.tone}`, { 'tray-toast': isToastWindow }]"
      :role="toastPayload.tone === 'error' ? 'alert' : 'status'"
      :aria-live="toastPayload.tone === 'error' ? 'assertive' : 'polite'"
      aria-atomic="true"
    >
      <span class="toast-card">
        <template v-if="isToastWindow">
          <img class="toast-app-icon" src="/cliproam-icon.png" alt="" aria-hidden="true" />
          <span class="toast-body">
            <span class="toast-title">ClipRoam</span>
            <span class="toast-message">{{ toastPayload.message }}</span>
          </span>
          <button class="toast-close" type="button" aria-label="关闭通知" @click="hideToastNow">
            <X :size="14" aria-hidden="true" />
          </button>
        </template>
        <template v-else>
          <CircleCheck v-if="toastPayload.tone === 'success'" :size="17" aria-hidden="true" />
          <CircleAlert v-else-if="toastPayload.tone === 'error'" :size="17" aria-hidden="true" />
          <Info v-else :size="17" aria-hidden="true" />
          <span>{{ toastPayload.message }}</span>
        </template>
      </span>
    </aside>
  </Transition>
</template>
