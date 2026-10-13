<script setup lang="ts">
import WorkspacePage from "./WorkspacePageLayout.vue";
import WorkspaceMenu from "./WorkspaceMenu.vue";
withDefaults(defineProps<{ title: string; layout?: "history" | "list" }>(), {layout:"list"});
</script>

<template>
  <WorkspacePage :title="title" :layout="layout">
    <template v-for="(_, name) in $slots" #[name]="slotProps">
      <slot :name="name" v-bind="slotProps" />
    </template>
    <template #menu><WorkspaceMenu /></template>
  </WorkspacePage>
</template>
