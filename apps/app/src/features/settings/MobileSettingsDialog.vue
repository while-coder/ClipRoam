<script setup lang="ts">
const emit = defineEmits<{ close: [] }>();
</script>

<template>
  <SModal :show="true" title="设置" width="400px" @update:show="emit('close')">
    <SEmpty class="mobile-settings-empty">暂无可配置项</SEmpty>
  </SModal>
</template>

<style scoped>
.mobile-settings-empty { min-height: 120px; padding: 24px 16px; }
</style>
