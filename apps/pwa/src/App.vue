<script setup lang="ts">
import { onBeforeUnmount, ref } from "vue";
import SharedApp from "@cliproam/ui/app";
import { provideMobileLayout } from "@cliproam/ui/platform";

const narrowViewport = window.matchMedia("(max-width: 768px)");
const mobileLayout = ref(narrowViewport.matches);
const updateLayout = (event: MediaQueryListEvent): void => {
  mobileLayout.value = event.matches;
};

provideMobileLayout(mobileLayout);
narrowViewport.addEventListener("change", updateLayout);
onBeforeUnmount(() => narrowViewport.removeEventListener("change", updateLayout));
</script>

<template><SharedApp /></template>
