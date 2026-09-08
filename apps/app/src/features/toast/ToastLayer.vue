<script setup lang="ts">
import { CircleAlert, CircleCheck, Info } from "lucide-vue-next";
import { isToastWindow } from "../../composables/usePlatform";
import { toastPayload } from "./useToast";
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
        <CircleCheck v-if="toastPayload.tone === 'success'" :size="17" aria-hidden="true" />
        <CircleAlert v-else-if="toastPayload.tone === 'error'" :size="17" aria-hidden="true" />
        <Info v-else :size="17" aria-hidden="true" />
        <span>{{ toastPayload.message }}</span>
      </span>
    </aside>
  </Transition>
</template>
