<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { confirm, toast } from "@qingfeng346/ui-kit";
import {
  deleteUser,
  deleteUserDevice,
  fetchUserDevices,
  fetchUsers,
  resetUserPassword,
  revokeUserDeviceSession,
  type AdminDevice,
  type AdminUser,
} from "../../shared/api.js";
import { errorMessage } from "../../shared/errorMessage.js";

const users = ref<AdminUser[]>([]);
const loading = ref(true);
const submitting = ref(false);
const error = ref("");
const search = ref("");

const resultSummary = computed(() => {
  if (loading.value) return "正在加载用户列表…";
  const total = `${users.value.length} 位用户`;
  return search.value.trim() ? `匹配「${search.value.trim()}」：${total}` : `共 ${total}`;
});

const searchDebounceMs = 250;
let searchTimer: ReturnType<typeof setTimeout> | undefined;

function onSearchInput(): void {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(load, searchDebounceMs);
}

async function load(): Promise<void> {
  loading.value = true;
  error.value = "";
  try {
    users.value = await fetchUsers(search.value);
  } catch (reason) {
    error.value = errorMessage(reason, "加载用户列表失败。");
  } finally {
    loading.value = false;
  }
}

async function removeUser(user: AdminUser): Promise<void> {
  if (submitting.value) return;
  const confirmed = await confirm.show({
    title: `删除用户 ${user.username}？`,
    content: "该用户的账号、会话与全部剪贴板数据都会被永久删除；已登录设备在下一次请求时会被拒绝。",
    confirmText: "确认删除",
    error: true,
  });
  if (!confirmed) return;
  submitting.value = true;
  error.value = "";
  try {
    await deleteUser(user.id);
    toast.show("success", `用户 ${user.username} 已删除。`);
    await load();
  } catch (reason) {
    error.value = errorMessage(reason, "删除用户失败。");
  } finally {
    submitting.value = false;
  }
}

const resettingUser = ref<AdminUser>();
const newPassword = ref("");
const passwordError = ref("");

function requestPasswordReset(user: AdminUser): void {
  error.value = "";
  passwordError.value = "";
  newPassword.value = "";
  resettingUser.value = user;
}

// Deliberately avoids look-alike characters (0/O, 1/l/I) so a generated
// password can be read out or handwritten without ambiguity.
function generatePassword(): void {
  const alphabet = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789";
  const values = crypto.getRandomValues(new Uint8Array(12));
  newPassword.value = Array.from(values, (value) => alphabet[value % alphabet.length]).join("");
}

async function confirmPasswordReset(): Promise<void> {
  const user = resettingUser.value;
  if (!user || submitting.value) return;
  if (newPassword.value.length < 6 || newPassword.value.length > 128) {
    passwordError.value = "新密码长度需在 6 到 128 位之间。";
    return;
  }
  submitting.value = true;
  passwordError.value = "";
  try {
    await resetUserPassword(user.id, newPassword.value);
    resettingUser.value = undefined;
    toast.show("success", `用户 ${user.username} 的密码已重置，该用户的所有会话已失效。`);
  } catch (reason) {
    passwordError.value = errorMessage(reason, "重置密码失败。");
  } finally {
    submitting.value = false;
  }
}

const managingDevicesUser = ref<AdminUser>();
const devices = ref<AdminDevice[]>([]);
const devicesLoading = ref(false);
const deviceError = ref("");
const removingDeviceId = ref("");
const signingOutDeviceId = ref("");

function requestDeviceManagement(user: AdminUser): void {
  error.value = "";
  deviceError.value = "";
  managingDevicesUser.value = user;
  devices.value = [];
  void loadDevices();
}

async function loadDevices(): Promise<void> {
  const user = managingDevicesUser.value;
  if (!user) return;
  devicesLoading.value = true;
  deviceError.value = "";
  try {
    devices.value = await fetchUserDevices(user.id);
  } catch (reason) {
    deviceError.value = errorMessage(reason, "加载设备列表失败。");
  } finally {
    devicesLoading.value = false;
  }
}

async function removeDevice(device: AdminDevice): Promise<void> {
  const user = managingDevicesUser.value;
  if (!user || removingDeviceId.value) return;
  removingDeviceId.value = device.id;
  deviceError.value = "";
  try {
    await deleteUserDevice(user.id, device.id);
    toast.show("success", `设备 ${device.name} 已删除，其同步条目一并清除。`);
    await loadDevices();
  } catch (reason) {
    deviceError.value = errorMessage(reason, "删除设备失败。");
  } finally {
    removingDeviceId.value = "";
  }
}

async function signOutDevice(device: AdminDevice): Promise<void> {
  const user = managingDevicesUser.value;
  if (!user || signingOutDeviceId.value) return;
  signingOutDeviceId.value = device.id;
  deviceError.value = "";
  try {
    await revokeUserDeviceSession(user.id, device.id);
    toast.show("success", `设备 ${device.name} 已下线，下次登录后恢复同步。`);
    await loadDevices();
  } catch (reason) {
    deviceError.value = errorMessage(reason, "强制下线失败。");
  } finally {
    signingOutDeviceId.value = "";
  }
}

// 二次确认：kit confirm 弹出嵌套确认（在设备管理 SModal 之上），确认后才真正调用接口。
async function requestDeviceAction(device: AdminDevice, action: "signOut" | "remove"): Promise<void> {
  deviceError.value = "";
  const confirmed = await confirm.show(
    action === "signOut"
      ? {
          title: `下线设备 ${device.name}？`,
          content: "该设备的登录会话将立即失效，设备下次登录后恢复同步；设备信息与同步条目不受影响。",
          confirmText: "确认下线",
        }
      : {
          title: `删除设备 ${device.name}？`,
          content:
            "将清除该设备的登录会话与设备信息，并删除它同步到服务器的全部剪贴板条目，其他设备将同步移除这些条目。此操作不可恢复。",
          confirmText: "确认删除",
          error: true,
        },
  );
  if (!confirmed) return;
  if (action === "signOut") await signOutDevice(device);
  else await removeDevice(device);
}

function formatDateTime(iso: string): string {
  return new Date(iso).toLocaleString();
}

/** 平台 · 系统版本 · 应用版本 拼成一行；未上报的应用版本不显示。 */
function deviceMeta(device: AdminDevice): string {
  return [
    device.platform,
    device.osVersion,
    device.appVersion && device.appVersion !== "未知" ? `v${device.appVersion}` : "",
  ].filter(Boolean).join(" · ");
}

onMounted(load);
onUnmounted(() => clearTimeout(searchTimer));
</script>

<template>
  <section class="content-section" aria-labelledby="users-title">
    <header class="topbar">
      <h1 id="users-title">用户管理</h1>
      <SButton :disabled="loading" @click="load">刷新</SButton>
    </header>

    <SCard title="注册用户">
      <template #header-extra>
        <SInput
          v-model:value="search"
          class="user-search"
          type="search"
          placeholder="搜索用户名"
          aria-label="搜索用户名"
          clearable
          @update:value="onSearchInput"
        />
      </template>

      <p class="muted panel-desc">删除用户会同时清除其账号、会话与剪贴板数据；其引用过的内容池文件会在下次回收时清理。</p>
      <p class="muted result-summary">{{ resultSummary }}</p>
      <SEmpty v-if="!loading && users.length === 0 && search.trim()">没有匹配「{{ search.trim() }}」的用户。</SEmpty>
      <SEmpty v-else-if="!loading && users.length === 0">还没有注册用户。</SEmpty>
      <STable v-else-if="!loading">
        <thead>
          <tr>
            <th scope="col">用户名</th>
            <th scope="col">注册时间</th>
            <th scope="col">活跃会话</th>
            <th scope="col"><span class="visually-hidden">操作</span></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="user in users" :key="user.id">
            <td>{{ user.username }}</td>
            <td>{{ formatDateTime(user.createdAt) }}</td>
            <td>{{ user.activeSessions }}</td>
            <td class="row-actions">
              <SButton :disabled="submitting" @click="requestDeviceManagement(user)">设备</SButton>
              <SButton :disabled="submitting" @click="requestPasswordReset(user)">重置密码</SButton>
              <SButton type="error" :disabled="submitting" @click="removeUser(user)">删除</SButton>
            </td>
          </tr>
        </tbody>
      </STable>

      <SAlert v-if="error" type="error">{{ error }}</SAlert>
    </SCard>

    <SModal
      :show="!!resettingUser"
      :title="resettingUser ? `重置 ${resettingUser.username} 的密码` : ''"
      width="sm"
      :closable="!submitting"
      :mask-closable="!submitting"
      :close-on-esc="!submitting"
      @update:show="(value: boolean) => { if (!value) resettingUser = undefined }"
    >
      <p>重置后该用户的所有会话立即失效，需要用新密码重新登录。</p>
      <form @submit.prevent="confirmPasswordReset">
        <label for="new-password">新密码</label>
        <div class="password-row">
          <SInput id="new-password" v-model:value="newPassword" type="text" autocomplete="off" spellcheck="false" maxlength="128" required autofocus :disabled="submitting" />
          <SButton :disabled="submitting" @click="generatePassword">随机生成</SButton>
        </div>
        <SAlert v-if="passwordError" type="error">{{ passwordError }}</SAlert>
      </form>
      <template #footer>
        <SButton :disabled="submitting" @click="resettingUser = undefined">取消</SButton>
        <SButton type="primary" :loading="submitting" @click="confirmPasswordReset">确认重置</SButton>
      </template>
    </SModal>

    <SModal
      :show="!!managingDevicesUser"
      :title="managingDevicesUser ? `${managingDevicesUser.username} 的设备` : ''"
      width="640px"
      @update:show="(value: boolean) => { if (!value) managingDevicesUser = undefined }"
    >
      <p>「下线」只清除该设备的登录会话，设备下次登录后恢复同步；「删除」清除登录会话、设备信息及其同步到服务器的全部剪贴板条目，其他设备将同步移除这些条目。</p>
      <p v-if="devicesLoading" class="muted"><span class="s-spinner" aria-hidden="true" /> 正在加载设备列表…</p>
      <SEmpty v-else-if="devices.length === 0">该用户还没有登录过任何设备。</SEmpty>
      <SList v-else bordered>
        <SListItem v-for="device in devices" :key="device.id">
          <div class="device-info">
            <strong :title="device.name">{{ device.name }}</strong>
            <span class="muted" :title="deviceMeta(device)">{{ deviceMeta(device) }}</span>
            <time class="muted" :datetime="device.lastSeenAt">最后登录 {{ formatDateTime(device.lastSeenAt) }}</time>
          </div>
          <template #suffix>
            <div class="device-actions">
              <SButton :disabled="!!signingOutDeviceId || !!removingDeviceId" @click="requestDeviceAction(device, 'signOut')">下线</SButton>
              <SButton type="error" :disabled="!!signingOutDeviceId || !!removingDeviceId" @click="requestDeviceAction(device, 'remove')">删除</SButton>
            </div>
          </template>
        </SListItem>
      </SList>
      <SAlert v-if="deviceError" type="error">{{ deviceError }}</SAlert>
      <template #footer>
        <SButton @click="managingDevicesUser = undefined">关闭</SButton>
      </template>
    </SModal>
  </section>
</template>