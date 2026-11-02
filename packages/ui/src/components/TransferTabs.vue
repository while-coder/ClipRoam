<script setup lang="ts">
import { nextTick } from "vue";
import { activeView } from "../accountSession";
import { transferTabs } from "./transferNavigation";

async function select(index: number): Promise<void> {
  activeView.value = transferTabs.value[index].view;
  await nextTick();
  document.getElementById(`transfer-tab-${activeView.value}`)?.focus();
}

function onKeydown(event: KeyboardEvent, index: number): void {
  let target: number;
  switch (event.key) {
    case "ArrowRight": target = (index + 1) % transferTabs.value.length; break;
    case "ArrowLeft": target = (index + transferTabs.value.length - 1) % transferTabs.value.length; break;
    case "Home": target = 0; break;
    case "End": target = transferTabs.value.length - 1; break;
    default: return;
  }
  event.preventDefault();
  void select(target);
}
</script>

<template>
  <div class="transfer-tabs" role="tablist" aria-label="传输类型">
    <button v-for="(tab, index) in transferTabs" :id="`transfer-tab-${tab.view}`" :key="tab.view"
      type="button" role="tab" :aria-selected="activeView === tab.view"
      :tabindex="activeView === tab.view ? 0 : -1" aria-controls="transfer-panel"
      @click="select(index)" @keydown="onKeydown($event, index)">
      {{ tab.label }}
      <span v-if="tab.count" class="transfer-tab-count">{{ tab.count > 99 ? '99+' : tab.count }}</span>
      <span v-if="tab.failed" class="transfer-tab-failed" aria-label="有失败任务" title="有失败任务">!</span>
    </button>
  </div>
</template>

<style scoped>
.transfer-tabs { display: flex; gap: 8px; padding: 0 20px; border-bottom: 1px solid var(--border); }
.transfer-tabs button { display: inline-flex; align-items: center; justify-content: center; gap: 6px; min-height: 44px; padding: 0 12px; border: 0; border-bottom: 2px solid transparent; background: transparent; color: var(--color-secondary-text); font: inherit; font-size: 13px; cursor: pointer; touch-action: manipulation; }
.transfer-tabs button:hover { background: var(--sui-bg-hover); }
.transfer-tabs button[aria-selected="true"] { color: var(--sui-primary); border-bottom-color: currentColor; }
.transfer-tabs button:focus-visible { outline: 2px solid var(--sui-primary); outline-offset: -3px; }
.transfer-tab-count { min-width: 18px; padding: 1px 5px; border-radius: 999px; background: var(--sui-bg-active); font-size: 11px; text-align: center; }
.transfer-tab-failed { color: var(--sui-error); font-weight: 700; }
:global(.mobile-app) .transfer-tabs { gap: 0; padding-inline: 12px; }
:global(.mobile-app) .transfer-tabs button { flex: 1; }
</style>
