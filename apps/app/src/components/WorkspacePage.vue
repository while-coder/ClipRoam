<script setup lang="ts">
import WorkspaceMenu from "./WorkspaceMenu.vue";
withDefaults(defineProps<{
  title: string;
  layout?: "history" | "list";
}>(), { layout: "list" });
</script>

<template>
  <section class="app-content history-content" :class="{ 'pending-content': layout === 'list', 'has-footer': !!$slots.footer }">
    <slot name="header">
      <header class="titlebar workspace-titlebar">
        <div class="page-title">
          <span :class="{ 'workspace-page-status': !!$slots.status }"><slot name="status">工作区</slot></span>
          <h1>{{ title }}</h1>
        </div>
        <div class="titlebar-actions"><slot name="actions" /><WorkspaceMenu /></div>
      </header>
    </slot>
    <slot name="toolbar" />
    <slot />
    <footer v-if="$slots.footer" class="footer-hint"><slot name="footer" /></footer>
  </section>
</template>
