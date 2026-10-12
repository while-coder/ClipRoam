<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Monitor, MonitorSmartphone, RefreshCw, Smartphone } from "lucide-vue-next";
import WorkspacePage from "../../components/WorkspacePage.vue";
import { activeAccountSession } from "../sync/accountSession";
import { connected, devicesById } from "../sync/syncEngine";
import { getDeviceIdentity } from "../../utils/device";
import { formatAge, formatExactDateTime } from "../../utils/format";
import type { Device } from "../../types";

defineProps<{ currentTime: number }>();

const currentDeviceId = ref("");
const loading = ref(false);
const refreshError = ref("");
let disposed = false;
let revision = 0;

const devices = computed(() => Object.values(devicesById.value).sort((a, b) =>
  Number(b.id === currentDeviceId.value) - Number(a.id === currentDeviceId.value)
  || (connected.value ? Number(b.online === true) - Number(a.online === true) : 0)
  || (b.lastOnlineAt ?? "").localeCompare(a.lastOnlineAt ?? "")
  || a.name.localeCompare(b.name, "zh-CN"),
));
const onlineCount = computed(() => devices.value.filter((device) => device.online === true).length);

function deviceIcon(device: Device) {
  if (/android|ios/i.test(device.platform)) return Smartphone;
  if (/windows|mac|linux/i.test(device.platform)) return Monitor;
  return MonitorSmartphone;
}

function statusText(device: Device): string {
  if (!connected.value || device.online === undefined) return "状态未知";
  return device.online ? "在线" : "离线";
}

async function refresh(): Promise<void> {
  const session = activeAccountSession.value;
  if (!session?.client) return;
  const requestRevision = ++revision;
  loading.value = true;
  refreshError.value = "";
  try {
    const success = await session.client.pullDevices();
    if (!disposed && requestRevision === revision && session === activeAccountSession.value && !success) {
      refreshError.value = "获取设备列表失败，请检查网络后重试。";
    }
  } finally {
    if (!disposed && requestRevision === revision) loading.value = false;
  }
}

watch([activeAccountSession, connected], () => { void refresh(); }, { immediate: true });
onMounted(async () => {
  try {
    const identity = await getDeviceIdentity();
    if (!disposed) currentDeviceId.value = identity.deviceId;
  } catch { /* Device information from the server remains available. */ }
});
onBeforeUnmount(() => { disposed = true; });
</script>

<template>
  <WorkspacePage title="设备">
    <template #status>
      <span role="status">共 {{ devices.length }} 台<template v-if="connected"> · {{ onlineCount }} 台在线</template></span>
    </template>
    <template #actions>
      <SButton :disabled="loading || !activeAccountSession?.client" @click="refresh">
        <span v-if="loading" class="s-spinner" aria-hidden="true" />
        <RefreshCw v-else :size="15" aria-hidden="true" />
        {{ loading ? '刷新中' : '刷新' }}
      </SButton>
    </template>

    <section class="device-list history-list" aria-label="设备列表" :aria-busy="loading">
      <div v-if="!connected" class="device-summary" role="status">同步连接已断开，设备状态暂不可确认</div>
      <p v-if="refreshError" class="device-error" role="alert">{{ refreshError }}</p>

      <article v-for="device in devices" :key="device.id" class="device-card">
        <div class="device-heading">
          <span class="kind-icon"><component :is="deviceIcon(device)" :size="22" aria-hidden="true" /></span>
          <div class="device-name">
            <h2>{{ device.name }}</h2>
            <STag v-if="device.id === currentDeviceId" type="info" size="small">当前设备</STag>
          </div>
          <span class="device-status" :class="{ online: connected && device.online === true }">
            <span class="status-dot" aria-hidden="true" />{{ statusText(device) }}
          </span>
        </div>
        <dl class="device-details">
          <div><dt>操作系统</dt><dd>{{ device.platform }} · {{ device.osVersion || '未知' }}</dd></div>
          <div><dt>应用版本</dt><dd>{{ device.appVersion || '未知' }}</dd></div>
          <div class="device-last-online">
            <dt>最后在线时间</dt>
            <dd v-if="connected && device.online === true">当前在线</dd>
            <dd v-else-if="device.lastOnlineAt">
              <time :datetime="device.lastOnlineAt">{{ formatExactDateTime(device.lastOnlineAt) }}</time>
              <span class="device-age">{{ formatAge(device.lastOnlineAt, currentTime) }}</span>
            </dd>
            <dd v-else>暂无记录</dd>
          </div>
          <div class="device-id"><dt>设备 ID</dt><dd>{{ device.id }}</dd></div>
        </dl>
      </article>
      <SEmpty v-if="!devices.length">{{ loading ? '正在获取设备列表…' : refreshError ? '设备列表暂不可用，请点击刷新重试' : '暂无设备，登录同一账号的设备会显示在这里' }}</SEmpty>
    </section>
  </WorkspacePage>
</template>

<style scoped>
.device-list { padding: 16px; }
.device-summary { display: flex; flex-wrap: wrap; gap: 8px 20px; margin-bottom: 16px; color: var(--color-secondary-text); font-size: 13px; line-height: 1.6; }
.device-card { padding: 16px; margin-bottom: 12px; border: 1px solid var(--border); border-radius: 10px; background: var(--color-surface); }
.device-heading { display: flex; align-items: center; gap: 12px; }
.device-heading .kind-icon { flex-shrink: 0; }
.device-name { display: flex; flex: 1; flex-wrap: wrap; align-items: center; gap: 8px; min-width: 0; }
.device-name h2 { margin: 0; font-size: 15px; font-weight: 600; overflow-wrap: anywhere; }
.device-status { display: flex; align-items: center; gap: 6px; flex-shrink: 0; color: var(--color-secondary-text); font-size: 12px; }
.device-status.online { color: var(--color-accent); }
.status-dot { width: 7px; height: 7px; background: currentColor; border-radius: 50%; }
.device-details { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 14px 24px; margin: 16px 0 0; font-size: 13px; line-height: 1.6; }
.device-details dt { margin-bottom: 3px; color: var(--color-secondary-text); font-size: 12px; }
.device-details dd { margin: 0; overflow-wrap: anywhere; }
.device-age { display: inline-block; margin-left: 8px; color: var(--color-secondary-text); font-size: 12px; }
.device-id dd { color: var(--color-secondary-text); font-family: ui-monospace, monospace; font-size: 12px; }
.device-error { color: var(--sui-error); font-size: 13px; line-height: 1.6; }
@media (max-width: 480px) {
  .device-list { padding: 12px; }
  .device-card { padding: 14px; }
  .device-details { gap: 12px; }
  .device-last-online, .device-id { grid-column: 1 / -1; }
}
</style>
