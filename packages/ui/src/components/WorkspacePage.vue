<script setup lang="ts">
import TransferTabs from "./TransferTabs.vue";

withDefaults(defineProps<{
  title: string;
  layout?: "history" | "list";
  transfer?: boolean;
}>(), { layout: "list" });
</script>

<template>
  <section class="app-content history-content" :class="{ 'pending-content': layout === 'list', 'has-footer': !!$slots.footer, 'transfer-content': transfer }">
    <slot name="header">
      <header class="titlebar workspace-titlebar">
        <div class="page-title">
          <span :class="{ 'workspace-page-status': !!$slots.status }"><slot name="status">工作区</slot></span>
          <slot name="title"><h1>{{ title }}</h1></slot>
        </div>
        <div class="titlebar-actions"><slot name="actions" /><slot name="menu" /></div>
      </header>
    </slot>
    <TransferTabs v-if="transfer" />
    <slot v-else name="toolbar" />
    <slot />
    <footer v-if="$slots.footer" class="footer-hint"><slot name="footer" /></footer>
  </section>
</template>
