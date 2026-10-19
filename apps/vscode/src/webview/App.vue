<script setup lang="ts">
import SharedApp from "@cliproam/ui/app";
import { onMounted } from "vue";
import { postMessage } from "./bridge";

onMounted(() => postMessage({ type: "ready" }));
</script>

<template><SharedApp /></template>
