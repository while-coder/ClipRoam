<script setup lang="ts">
import { computed, ref } from "vue";
import { WorkspaceLayout, WorkspacePage, type WorkspaceView } from "@cliproam/ui";

const activeView = ref<WorkspaceView>("history");
const titles: Record<WorkspaceView,string> = {history:"剪贴板历史","pending-upload":"待上传",uploads:"上传",downloads:"下载",devices:"设备"};
const title = computed(() => titles[activeView.value]);
</script>

<template>
  <WorkspaceLayout v-model:active-view="activeView">
    <WorkspacePage :title="title">
      <template #status>ClipRoam Chrome 插件 · 占位项目</template>
      <div class="empty-state" role="status">
        <strong>{{ title }}</strong>
        <span>Chrome 插件项目已接入共用主界面，业务功能后续接入。</span>
      </div>
    </WorkspacePage>
  </WorkspaceLayout>
</template>
