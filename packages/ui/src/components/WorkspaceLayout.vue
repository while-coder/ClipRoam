<script setup lang="ts">
import { appName } from "../branding";
import { computed, ref, watch } from "vue";
import { Clipboard, Cloud, CloudOff, ArrowDownUp, MonitorSmartphone, Star } from "lucide-vue-next";
import type { WorkspaceView } from "../types";
import { hasTransferFailure, transferTabs } from "./transferNavigation";
import SidebarResizer from "./SidebarResizer.vue";

const props = withDefaults(defineProps<{
  activeView: WorkspaceView;
  mobile?: boolean;
  quickPaste?: boolean;
  pendingCount?: number;
  activeUploadCount?: number;
  activeDownloadCount?: number;
  connected?: boolean;
  connectionStatus?: { tone: string; label: string; title: string };
}>(), {
  mobile: false, quickPaste: false, pendingCount: 0, activeUploadCount: 0, activeDownloadCount: 0, connected: false,
  connectionStatus: () => ({tone:"",label:"未连接",title:"未连接服务器"}),
});
const emit = defineEmits<{ "update:activeView": [view: WorkspaceView] }>();
const lastTransferView = ref<WorkspaceView>("pending-upload");
const sidebarWidth = ref(208);
const transferActive = computed(() => transferTabs.value.some((tab) => tab.view === props.activeView));
const transferCount = computed(() => props.pendingCount + props.activeUploadCount + props.activeDownloadCount);
watch(() => props.activeView, (view) => {
  if (transferTabs.value.some((tab) => tab.view === view)) lastTransferView.value = view;
}, { immediate: true, flush: "sync" });
const mobilePages = computed(() => [
  {view:"history" as const,label:"历史",icon:Clipboard,count:0,active:props.activeView === "history",failed:false},
  {view:"favorites" as const,label:"收藏夹",icon:Star,count:0,active:props.activeView === "favorites",failed:false},
  {view:lastTransferView.value,label:"传输",icon:ArrowDownUp,count:transferCount.value,active:transferActive.value,failed:hasTransferFailure.value},
  {view:"devices" as const,label:"设备",icon:MonitorSmartphone,count:0,active:props.activeView === "devices",failed:false},
]);
</script>

<template>
  <main class="app-shell" :class="{ 'paste-app': quickPaste, 'mobile-app': mobile }" :style="{ '--sidebar-width': `${sidebarWidth}px` }">
    <aside v-if="!mobile" v-show="!quickPaste" class="sidebar" aria-label="主导航">
      <header class="sidebar-brand">
        <span class="brand-mark"><Clipboard :size="17" /></span>
        <span>
          <strong>{{ appName }}</strong>
          <small>剪贴板工作区</small>
        </span>
      </header>

      <nav class="sidebar-nav" aria-label="功能模块">
        <span class="nav-section-label">工作区</span>
        <button
          class="nav-item"
          :class="{ active: activeView === 'history' }"
          type="button"
          :aria-current="activeView === 'history' ? 'page' : undefined"
          @click="emit('update:activeView', 'history')"
        >
          <Clipboard :size="17" aria-hidden="true" />
          <span>剪贴板历史</span>
        </button>
        <button class="nav-item" :class="{ active: activeView === 'favorites' }" type="button"
          :aria-current="activeView === 'favorites' ? 'page' : undefined" @click="emit('update:activeView', 'favorites')">
          <Star :size="17" aria-hidden="true" /><span>收藏夹</span>
        </button>
        <button
          class="nav-item"
          :class="{ active: transferActive }"
          type="button"
          :aria-current="transferActive ? 'page' : undefined"
          @click="emit('update:activeView', lastTransferView)"
        >
          <ArrowDownUp :size="17" aria-hidden="true" />
          <span>传输</span>
          <span v-if="hasTransferFailure" class="transfer-failure" aria-label="有失败任务" title="有失败任务">!</span>
          <span v-if="transferCount" class="nav-count">{{ transferCount > 99 ? '99+' : transferCount }}</span>
        </button>
        <button
          class="nav-item"
          :class="{ active: activeView === 'devices' }"
          type="button"
          :aria-current="activeView === 'devices' ? 'page' : undefined"
          @click="emit('update:activeView', 'devices')"
        >
          <MonitorSmartphone :size="17" aria-hidden="true" />
          <span>设备</span>
        </button>
      </nav>

      <div class="sidebar-bottom">
        <div class="sidebar-status" :class="connectionStatus.tone" :title="connectionStatus.title">
          <Cloud v-if="connected" :size="15" aria-hidden="true" />
          <CloudOff v-else :size="15" aria-hidden="true" />
          <span>{{ connectionStatus.label }}</span>
        </div>
      </div>
    </aside>

    <SidebarResizer v-if="!mobile && !quickPaste" storage-key="cliproam.sidebarWidth" label="主菜单宽度"
      :default-width="208" :min-width="176" :max-width="360" :content-min-width="480" @resize="sidebarWidth = $event" />
    <slot />

    <nav v-if="mobile && !quickPaste" class="mobile-navigation" :style="{ gridTemplateColumns: `repeat(${mobilePages.length}, minmax(0, 1fr))`, gap: '2px' }" aria-label="主导航">
      <button
        v-for="page in mobilePages"
        :key="page.view"
        type="button"
        class="mobile-nav-item"
        :class="{ active: page.active }"
        :aria-current="page.active ? 'page' : undefined"
        @click="emit('update:activeView', page.view)"
      >
        <span class="mobile-nav-icon">
          <component :is="page.icon" :size="18" aria-hidden="true" />
          <span v-if="page.count" class="mobile-nav-count" :aria-label="`${page.count} 个任务`">{{ page.count > 99 ? '99+' : page.count }}</span>
        </span>
        <span>{{ page.label }}</span>
        <span v-if="page.failed" class="transfer-failure mobile-transfer-failure" aria-label="有失败任务" title="有失败任务">!</span>
      </button>
    </nav>

    <slot name="overlays" />
  </main>
</template>
