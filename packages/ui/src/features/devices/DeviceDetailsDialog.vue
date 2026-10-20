<script setup lang="ts">
import { computed } from "vue";
import type { Device } from "../../types";
import { formatExactDateTime, formatFileSize } from "../../utils";

const props = defineProps<{ device: Device; connected: boolean; currentDevice: boolean }>();
const emit = defineEmits<{ close: [] }>();
const status = computed(() => !props.connected || props.device.online === undefined
  ? "状态未知" : props.device.online ? "在线" : "离线");
</script>

<template>
  <SModal :show="true" title="设备详细信息" width="560px"
    @update:show="(value: boolean) => { if (!value) emit('close') }">
    <dl class="device-info">
      <div><dt>设备别名</dt><dd>{{ device.deviceAlias || '未设置' }} <STag v-if="currentDevice" type="info" size="small">当前设备</STag></dd></div>
      <div><dt>系统设备名</dt><dd>{{ device.deviceName || '未知' }}</dd></div>
      <div><dt>设备状态</dt><dd><span class="device-status" :class="{ online: connected && device.online === true, offline: connected && device.online === false }"><span class="status-dot" aria-hidden="true" />{{ status }}</span></dd></div>
      <div v-if="device.deviceBrand"><dt>设备品牌</dt><dd>{{ device.deviceBrand }}</dd></div>
      <div v-if="device.deviceMarketName"><dt>商品型号</dt><dd>{{ device.deviceMarketName }}</dd></div>
      <div><dt>系统型号</dt><dd>{{ device.deviceModel || '未知' }}</dd></div>
      <div><dt>处理器</dt><dd>{{ device.cpu || '未知' }}</dd></div>
      <div><dt>处理器架构</dt><dd>{{ device.architecture || '未知' }}</dd></div>
      <div><dt>物理内存</dt><dd>{{ device.memoryTotalBytes ? formatFileSize(device.memoryTotalBytes) : '未知' }}</dd></div>
      <div><dt>系统 / 浏览器</dt><dd>{{ device.platform || '未知' }}</dd></div>
      <div><dt>系统 / 浏览器版本</dt><dd>{{ device.osVersion || '未知' }}</dd></div>
      <div v-if="device.androidApiLevel"><dt>Android API Level</dt><dd>{{ device.androidApiLevel }}</dd></div>
      <div><dt>应用版本</dt><dd>{{ device.appVersion || '未知' }}</dd></div>
      <div v-if="device.appBuildNumber"><dt>应用构建号</dt><dd>{{ device.appBuildNumber }}</dd></div>
      <div><dt>系统用户名</dt><dd>{{ device.username || '未知' }}</dd></div>
      <div><dt>首选语言</dt><dd>{{ device.language || '未知' }}</dd></div>
      <div><dt>系统区域设置</dt><dd>{{ device.systemLocale || '未知' }}</dd></div>
      <div><dt>时区</dt><dd>{{ device.timeZone || '未知' }}</dd></div>
      <div>
        <dt>最后在线时间</dt>
        <dd v-if="connected && device.online === true">当前在线</dd>
        <dd v-else-if="device.lastOnlineAt"><time :datetime="device.lastOnlineAt">{{ formatExactDateTime(device.lastOnlineAt) }}</time></dd>
        <dd v-else>暂无记录</dd>
      </div>
      <div><dt>设备 ID</dt><dd class="device-info-id">{{ device.id }}</dd></div>
    </dl>
    <template #footer><SButton @click="emit('close')">关闭</SButton></template>
  </SModal>
</template>

<style scoped>
.device-info { margin: 0; font-size: 14px; line-height: 1.6; }
.device-info > div { display: grid; grid-template-columns: minmax(0, 140px) minmax(0, 1fr); gap: 8px 16px; padding: 10px 0; border-bottom: 1px solid var(--border); }
.device-info > div:last-child { border-bottom: 0; }
.device-info dt { color: var(--color-secondary-text); }
.device-info dd { min-width: 0; margin: 0; overflow-wrap: anywhere; white-space: pre-wrap; }
.device-info-id { font-family: ui-monospace, monospace; font-size: 12px; }
.device-status { display: inline-flex; align-items: center; gap: 6px; padding: 2px 8px; border: 1px solid var(--border); border-radius: 999px; background: var(--sui-bg-hover); color: var(--color-secondary-text); font-size: 12px; line-height: 1.5; }
.device-status.online { color: var(--color-accent); border-color: currentColor; }
.status-dot { width: 7px; height: 7px; background: currentColor; border-radius: 50%; }
.device-status.offline .status-dot { background: transparent; box-shadow: inset 0 0 0 1px currentColor; }
@media (max-width: 480px) {
  .device-info > div { grid-template-columns: minmax(0, 1fr); gap: 2px; }
}
</style>
