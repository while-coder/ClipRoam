<script setup lang="ts">
import { errorMessage, formatAge, formatExactDateTime } from "../../utils";
import { clientSessionManager } from "../sync/clientSessionManager";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Monitor, MonitorSmartphone, RefreshCw, Smartphone, Trash2 } from "lucide-vue-next";
import WorkspacePage from "../../components/WorkspacePage.vue";
import WorkspaceMenu from "../../components/WorkspaceMenu.vue";
import { activeAccountSession } from "../../accountSession";
import { getDeviceIdentity } from "../../platform/storage";
import type { Device } from "../../types";
import { confirm, toast } from "@qingfeng346/ui-kit";

const { connected, devicesById } = clientSessionManager;

defineProps<{ currentTime: number }>();

const currentDeviceId = ref("");
const loading = ref(false);
const refreshError = ref("");
const removingDeviceId = ref("");
const removeError = ref("");
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

async function removeDevice(device: Device): Promise<void> {
  const session = activeAccountSession.value;
  if (!session?.client || !currentDeviceId.value || device.id === currentDeviceId.value || removingDeviceId.value) return;
  removingDeviceId.value = device.id;
  removeError.value = "";
  try {
    const confirmed = await confirm.show({
      title: `移除设备 ${device.name}？`,
      content: "将注销该设备并清除设备信息，同时删除它上传到服务器的全部剪贴板历史，其他设备也会同步移除这些历史。此操作不可恢复。该设备重新登录后可再次加入。",
      confirmText: "确认移除",
      error: true,
    });
    if (!confirmed || disposed || session !== activeAccountSession.value) return;
    await session.client.http.request("DELETE", `/devices/${encodeURIComponent(device.id)}`,
      { signal: AbortSignal.timeout(30_000) }, null, "");
    if (disposed || session !== activeAccountSession.value) return;
    const remaining = { ...session.state.devicesById.value };
    delete remaining[device.id];
    session.state.devicesById.value = remaining;
    toast.show("success", `设备 ${device.name} 已移除。`);
    await refresh();
  } catch (error) {
    if (!disposed && session === activeAccountSession.value) removeError.value = `移除设备失败：${errorMessage(error)}`;
  } finally {
    removingDeviceId.value = "";
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
    <template #menu><WorkspaceMenu /></template>
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
      <p v-if="removeError" class="device-error" role="alert">{{ removeError }}</p>

      <article v-for="device in devices" :key="device.id" class="device-card">
        <div class="device-heading">
          <span class="kind-icon"><component :is="deviceIcon(device)" :size="18" aria-hidden="true" /></span>
          <div class="device-name">
            <h2>{{ device.name }}</h2>
            <STag v-if="device.id === currentDeviceId" type="info" size="small">当前设备</STag>
          </div>
          <span class="device-status" :class="{ online: connected && device.online === true }">
            <span class="status-dot" aria-hidden="true" />{{ statusText(device) }}
          </span>
          <button v-if="currentDeviceId && device.id !== currentDeviceId" type="button" class="device-remove"
            :disabled="!!removingDeviceId || !connected || !activeAccountSession?.client"
            :aria-label="removingDeviceId === device.id ? `正在处理设备 ${device.name}` : `移除设备 ${device.name}`"
            :aria-busy="removingDeviceId === device.id"
            :title="removingDeviceId === device.id ? '处理中' : '移除设备'" @click="removeDevice(device)">
            <span v-if="removingDeviceId === device.id" class="s-spinner" aria-hidden="true" />
            <Trash2 v-else :size="15" aria-hidden="true" />
          </button>
        </div>
        <dl class="device-details">
          <div><dt>系统 / 浏览器</dt><dd>{{ device.platform }} · {{ device.osVersion || '未知' }}</dd></div>
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
.device-card { padding: 10px 12px; margin-bottom: 8px; border: 1px solid var(--border); border-radius: 8px; background: var(--color-surface); }
.device-heading { display: flex; align-items: center; gap: 8px; }
.device-heading .kind-icon { width: 28px; height: 28px; border-radius: 7px; flex-shrink: 0; }
.device-name { display: flex; flex: 1; flex-wrap: wrap; align-items: center; gap: 8px; min-width: 0; }
.device-name h2 { margin: 0; font-size: 14px; line-height: 1.4; font-weight: 600; overflow-wrap: anywhere; }
.device-status { display: flex; align-items: center; gap: 6px; flex-shrink: 0; color: var(--color-secondary-text); font-size: 12px; }
.device-status.online { color: var(--color-accent); }
.status-dot { width: 7px; height: 7px; background: currentColor; border-radius: 50%; }
.device-details { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, .7fr) minmax(0, 1.5fr); gap: 4px 16px; margin: 8px 0 0; font-size: 13px; line-height: 1.5; }
.device-details > div { display: flex; align-items: baseline; gap: 8px; min-width: 0; }
.device-details dt { flex-shrink: 0; color: var(--color-secondary-text); font-size: 12px; }
.device-details dd { min-width: 0; margin: 0; overflow-wrap: anywhere; }
.device-age { display: inline-block; margin-left: 8px; color: var(--color-secondary-text); font-size: 12px; }
.device-id { grid-column: 1 / -1; }
.device-id dd { color: var(--color-secondary-text); font-family: ui-monospace, monospace; font-size: 12px; }
.device-error { color: var(--sui-error); font-size: 13px; line-height: 1.6; }
.device-remove { display: grid; place-items: center; flex-shrink: 0; width: 28px; height: 28px; padding: 0; border: 0; border-radius: 6px; background: transparent; color: var(--color-secondary-text); cursor: pointer; touch-action: manipulation; transition: color 150ms, background-color 150ms; }
.device-remove:hover:not(:disabled), .device-remove:focus-visible { background: var(--sui-bg-hover); color: var(--sui-error); }
.device-remove:active:not(:disabled) { background: var(--sui-bg-active); }
.device-remove:focus-visible { outline: 2px solid var(--color-accent); outline-offset: 2px; }
.device-remove:disabled { opacity: .4; cursor: default; }
@media (pointer: coarse) {
  .device-remove { width: 44px; height: 44px; }
}
@media (prefers-reduced-motion: reduce) {
  .device-remove { transition: none; }
}
@media (max-width: 760px) {
  .device-details { grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 4px 12px; }
  .device-last-online { grid-column: 1 / -1; }
}
@media (max-width: 480px) {
  .device-list { padding: 12px; }
  .device-card { padding: 10px; }
  .device-details { grid-template-columns: minmax(0, 1fr); }
}
</style>
