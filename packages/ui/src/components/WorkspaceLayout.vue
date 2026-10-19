<script setup lang="ts">
import { appName } from "../branding";
import { computed } from "vue";
import { Clipboard, Cloud, CloudOff, CloudUpload, Download, MonitorSmartphone, Upload } from "lucide-vue-next";
import type { WorkspaceView } from "../types";

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
const mobilePages = computed(() => [
  {view:"history" as const,label:"历史",icon:Clipboard,count:0},
  {view:"pending-upload" as const,label:"待上传",icon:CloudUpload,count:props.pendingCount},
  {view:"uploads" as const,label:"上传",icon:Upload,count:props.activeUploadCount},
  {view:"downloads" as const,label:"下载",icon:Download,count:props.activeDownloadCount},
  {view:"devices" as const,label:"设备",icon:MonitorSmartphone,count:0},
]);
</script>

<template>
  <main class="app-shell" :class="{ 'paste-app': quickPaste, 'mobile-app': mobile }">
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
        <button
          class="nav-item"
          :class="{ active: activeView === 'pending-upload' }"
          type="button"
          :aria-current="activeView === 'pending-upload' ? 'page' : undefined"
          @click="emit('update:activeView', 'pending-upload')"
        >
          <CloudUpload :size="17" aria-hidden="true" />
          <span>待上传</span>
          <span v-if="pendingCount" class="nav-count">{{ pendingCount }}</span>
        </button>
        <button
          class="nav-item"
          :class="{ active: activeView === 'uploads' }"
          type="button"
          :aria-current="activeView === 'uploads' ? 'page' : undefined"
          @click="emit('update:activeView', 'uploads')"
        >
          <Upload :size="17" aria-hidden="true" />
          <span>上传</span>
          <span v-if="activeUploadCount" class="nav-count">{{ activeUploadCount }}</span>
        </button>
        <button
          class="nav-item"
          :class="{ active: activeView === 'downloads' }"
          type="button"
          :aria-current="activeView === 'downloads' ? 'page' : undefined"
          @click="emit('update:activeView', 'downloads')"
        >
          <Download :size="17" aria-hidden="true" />
          <span>下载</span>
          <span v-if="activeDownloadCount" class="nav-count">{{ activeDownloadCount }}</span>
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

    <slot />

    <nav v-if="mobile && !quickPaste" class="mobile-navigation" aria-label="主导航">
      <button
        v-for="page in mobilePages"
        :key="page.view"
        type="button"
        class="mobile-nav-item"
        :class="{ active: activeView === page.view }"
        :aria-current="activeView === page.view ? 'page' : undefined"
        @click="emit('update:activeView', page.view)"
      >
        <span class="mobile-nav-icon">
          <component :is="page.icon" :size="18" aria-hidden="true" />
          <span v-if="page.count" class="mobile-nav-count" :aria-label="`${page.count} 个任务`">{{ page.count > 99 ? '99+' : page.count }}</span>
        </span>
        <span>{{ page.label }}</span>
      </button>
    </nav>

    <slot name="overlays" />
  </main>
</template>
