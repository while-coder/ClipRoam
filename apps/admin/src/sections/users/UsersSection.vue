<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import {
  deleteUser,
  deleteUserDevice,
  fetchUserDevices,
  fetchUsers,
  resetUserPassword,
  type AdminDevice,
  type AdminUser,
} from "../../shared/api.js";

const users = ref<AdminUser[]>([]);
const loading = ref(true);
const submitting = ref(false);
const error = ref("");
const notice = ref("");
const search = ref("");
const confirmingUser = ref<AdminUser>();

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
    error.value = reason instanceof Error ? reason.message : "加载用户列表失败。";
  } finally {
    loading.value = false;
  }
}

function requestRemoval(user: AdminUser): void {
  error.value = "";
  notice.value = "";
  confirmingUser.value = user;
}

async function removeUser(): Promise<void> {
  const user = confirmingUser.value;
  if (!user || submitting.value) return;
  submitting.value = true;
  error.value = "";
  notice.value = "";
  try {
    await deleteUser(user.id);
    confirmingUser.value = undefined;
    notice.value = `用户 ${user.username} 已删除。`;
    await load();
  } catch (reason) {
    error.value = reason instanceof Error ? reason.message : "删除用户失败。";
  } finally {
    submitting.value = false;
  }
}

const resettingUser = ref<AdminUser>();
const newPassword = ref("");
const passwordError = ref("");

function requestPasswordReset(user: AdminUser): void {
  error.value = "";
  notice.value = "";
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
    notice.value = `用户 ${user.username} 的密码已重置，该用户的所有会话已失效。`;
  } catch (reason) {
    passwordError.value = reason instanceof Error ? reason.message : "重置密码失败。";
  } finally {
    submitting.value = false;
  }
}

const managingDevicesUser = ref<AdminUser>();
const devices = ref<AdminDevice[]>([]);
const devicesLoading = ref(false);
const deviceError = ref("");
const removingDeviceId = ref("");

function requestDeviceManagement(user: AdminUser): void {
  error.value = "";
  notice.value = "";
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
    deviceError.value = reason instanceof Error ? reason.message : "加载设备列表失败。";
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
    await loadDevices();
  } catch (reason) {
    deviceError.value = reason instanceof Error ? reason.message : "删除设备失败。";
  } finally {
    removingDeviceId.value = "";
  }
}

function formatDateTime(iso: string): string {
  return new Date(iso).toLocaleString();
}

onMounted(load);
onUnmounted(() => clearTimeout(searchTimer));
</script>

<template>
  <section class="content-section" aria-labelledby="users-title">
    <header class="topbar">
      <h1 id="users-title">用户管理</h1>
      <button class="secondary" type="button" :disabled="loading" @click="load">刷新</button>
    </header>

    <section class="panel">
      <div class="panel-heading">
        <div>
          <h2>注册用户</h2>
          <p>删除用户会同时清除其账号、会话与剪贴板数据；其引用过的内容池文件会在下次回收时清理。</p>
        </div>
        <input
          v-model="search"
          class="user-search"
          type="search"
          placeholder="搜索用户名"
          aria-label="搜索用户名"
          @input="onSearchInput"
        />
      </div>

      <p class="muted result-summary">{{ resultSummary }}</p>
      <p v-if="!loading && users.length === 0 && search.trim()" class="muted">没有匹配「{{ search.trim() }}」的用户。</p>
      <p v-else-if="!loading && users.length === 0" class="muted">还没有注册用户。</p>
      <table v-else-if="!loading" class="user-table">
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
              <button class="secondary" type="button" :disabled="submitting" @click="requestDeviceManagement(user)">设备</button>
              <button class="secondary" type="button" :disabled="submitting" @click="requestPasswordReset(user)">重置密码</button>
              <button class="danger" type="button" :disabled="submitting" @click="requestRemoval(user)">删除</button>
            </td>
          </tr>
        </tbody>
      </table>

      <p v-if="error" class="message error" role="alert">{{ error }}</p>
      <p v-if="notice" class="message success" role="status">{{ notice }}</p>
    </section>

    <div v-if="confirmingUser" class="modal-backdrop" role="presentation">
      <section class="confirm-dialog" role="dialog" aria-modal="true" aria-labelledby="delete-user-title">
        <h2 id="delete-user-title">删除用户 {{ confirmingUser.username }}？</h2>
        <p>该用户的账号、会话与全部剪贴板数据都会被永久删除；已登录设备在下一次请求时会被拒绝。</p>
        <p v-if="error" class="message error" role="alert">{{ error }}</p>
        <div class="form-actions">
          <button class="secondary" type="button" :disabled="submitting" @click="confirmingUser = undefined">取消</button>
          <button class="danger" type="button" :disabled="submitting" @click="removeUser">{{ submitting ? "正在删除…" : "确认删除" }}</button>
        </div>
      </section>
    </div>

    <div v-if="resettingUser" class="modal-backdrop" role="presentation">
      <section class="confirm-dialog" role="dialog" aria-modal="true" aria-labelledby="reset-password-title">
        <h2 id="reset-password-title">重置 {{ resettingUser.username }} 的密码</h2>
        <p>重置后该用户的所有会话立即失效，需要用新密码重新登录。</p>
        <form @submit.prevent="confirmPasswordReset">
          <label for="new-password">新密码</label>
          <div class="password-row">
            <input id="new-password" v-model="newPassword" type="text" autocomplete="off" spellcheck="false" maxlength="128" required autofocus />
            <button class="secondary" type="button" :disabled="submitting" @click="generatePassword">随机生成</button>
          </div>
          <p v-if="passwordError" class="message error" role="alert">{{ passwordError }}</p>
          <div class="form-actions">
            <button class="secondary" type="button" :disabled="submitting" @click="resettingUser = undefined">取消</button>
            <button type="submit" :disabled="submitting">{{ submitting ? "正在重置…" : "确认重置" }}</button>
          </div>
        </form>
      </section>
    </div>

    <div v-if="managingDevicesUser" class="modal-backdrop" role="presentation">
      <section class="confirm-dialog devices-dialog" role="dialog" aria-modal="true" aria-labelledby="devices-title">
        <h2 id="devices-title">{{ managingDevicesUser.username }} 的设备</h2>
        <p>删除设备后，该设备的登录会话同时失效，需要重新登录才能继续同步。</p>
        <p v-if="devicesLoading" class="muted">正在加载设备列表…</p>
        <p v-else-if="devices.length === 0" class="muted">该用户还没有登录过任何设备。</p>
        <ul v-else class="device-list">
          <li v-for="device in devices" :key="device.id" class="device-item">
            <div class="device-info">
              <strong>{{ device.name }}</strong>
              <span class="muted">{{ device.platform }} · {{ device.osVersion }}</span>
            </div>
            <div class="device-side">
              <time class="muted" :title="'最后登录时间'">最后登录 {{ formatDateTime(device.lastSeenAt) }}</time>
              <button class="danger" type="button" :disabled="!!removingDeviceId" @click="removeDevice(device)">
                {{ removingDeviceId === device.id ? "正在删除…" : "删除" }}
              </button>
            </div>
          </li>
        </ul>
        <p v-if="deviceError" class="message error" role="alert">{{ deviceError }}</p>
        <div class="form-actions">
          <button class="secondary" type="button" @click="managingDevicesUser = undefined">关闭</button>
        </div>
      </section>
    </div>
  </section>
</template>
