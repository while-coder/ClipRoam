<script setup lang="ts">
import { computed } from "vue";
import { clientSessionManager } from "../sync/clientSessionManager";

const limits = computed(() => clientSessionManager.getActivePreferences());
const connected = clientSessionManager.connected;
</script>

<template>
  <SCard title="服务器限制">
    <p class="settings-section-desc">由服务器管理员设置，适用于当前账号。</p>
    <dl class="server-limits">
      <div>
        <dt>普通文件存储</dt>
        <dd>{{ limits.serverMaxFileMb === 0 ? "禁止保存" : `单个文件小于 ${limits.serverMaxFileMb} MB` }}</dd>
      </div>
      <div>
        <dt>剪贴板图片存储</dt>
        <dd>{{ limits.serverMaxImageMb === 0 ? "禁止保存" : `单张图片小于 ${limits.serverMaxImageMb} MB，自动上传` }}</dd>
      </div>
      <div>
        <dt>单账号历史条数</dt>
        <dd>最多 {{ limits.serverMaxHistoryEntries }} 条</dd>
      </div>
      <div>
        <dt>单次复制文件数</dt>
        <dd>最多 {{ limits.maxCaptureFileCount }} 个</dd>
      </div>
    </dl>
    <span v-if="!connected" class="field-hint">同步连接已断开，当前显示上次获取的服务器配置。</span>
  </SCard>
</template>

<style scoped>
.server-limits { margin: 0; }
.server-limits > div { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 4px 16px; padding: 8px 0; }
.server-limits dt { font-weight: 500; }
.server-limits dd { margin: 0; overflow-wrap: anywhere; }
</style>
