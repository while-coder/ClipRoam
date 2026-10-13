<script setup lang="ts">
import { onBeforeUnmount, onMounted } from "vue";
import ToastLayer from "./ToastLayer.vue";
import { disposeToast, startToastWindowListener } from "./useToastWindow";
import type { Unlisten } from "@cliproam/ui/native";
import "./styles.css";

document.documentElement.classList.add("toast-window-root");
let unlisten: Unlisten | undefined;
onMounted(async () => { unlisten = await startToastWindowListener(); });
onBeforeUnmount(() => { unlisten?.(); disposeToast(); });
</script>

<template>
  <main class="toast-window-shell" aria-hidden="true"></main>
  <ToastLayer />
</template>
