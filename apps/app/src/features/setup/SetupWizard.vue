<script lang="ts">
import type { AuthMode, ServerProtocol } from "../sync/syncClient";

/** 提交给 App.vue 的表单草稿；serverAddress 已经过规范化。 */
export type SetupDraft = {
  serverAddress: string;
  serverProtocol: ServerProtocol;
  username: string;
  password: string;
  authMode: AuthMode;
};
</script>

<script setup lang="ts">
import { ref } from "vue";
import { ArrowLeft, LoaderCircle, Server, ShieldCheck } from "lucide-vue-next";
import { normalizeServerAddress } from "../sync/syncClient";
import { errorMessage } from "../../utils/error";
import { CONFIGURED_SERVER_PROTOCOL, DEFAULT_SERVER_ADDRESS } from "../../utils/constants";
import type { SyncConfig } from "../../types";

defineProps<{
  hasSavedSyncConfig: boolean;
  busy: boolean;
  error: string;
}>();

const emit = defineEmits<{
  submit: [draft: SetupDraft];
  local: [draft: SetupDraft];
  close: [];
  "reset-error": [];
}>();

const setupServerAddress = ref(DEFAULT_SERVER_ADDRESS);
const setupServerProtocol = ref<ServerProtocol>(CONFIGURED_SERVER_PROTOCOL);
const setupUsername = ref("");
const setupPassword = ref("");
const authMode = ref<AuthMode>("login");
const serverFieldError = ref("");
const usernameFieldError = ref("");
const passwordFieldError = ref("");
const serverInput = ref<HTMLInputElement>();
const accountPasswordInput = ref<HTMLInputElement>();

function setFields(config?: SyncConfig): void {
  setupServerAddress.value = config?.serverAddress || DEFAULT_SERVER_ADDRESS;
  setupServerProtocol.value = config?.serverProtocol || CONFIGURED_SERVER_PROTOCOL;
  setupUsername.value = config?.username || "";
  setupPassword.value = "";
  authMode.value = "login";
  serverFieldError.value = "";
  usernameFieldError.value = "";
  passwordFieldError.value = "";
}

function validateServerField(): string | undefined {
  try {
    const normalized = normalizeServerAddress(setupServerAddress.value);
    setupServerAddress.value = normalized;
    serverFieldError.value = "";
    return normalized;
  } catch (error) {
    serverFieldError.value = errorMessage(error);
    return undefined;
  }
}

function validateUsernameField(): boolean {
  usernameFieldError.value = /^[a-zA-Z0-9_.-]{3,32}$/.test(setupUsername.value.trim())
    ? ""
    : "账号需为 3-32 位字母、数字或 _.-";
  return !usernameFieldError.value;
}

function validatePasswordField(): boolean {
  const length = setupPassword.value.length;
  passwordFieldError.value = length >= 6 && length <= 128 ? "" : "密码长度需为 6-128 位";
  return !passwordFieldError.value;
}

function switchAuthMode(mode: AuthMode): void {
  authMode.value = mode;
  emit("reset-error");
  usernameFieldError.value = "";
  passwordFieldError.value = "";
}

function submit(): void {
  // 先校验再 emit：validateServerField 会回写规范化后的地址，
  // 保证父组件拿到的是规范化 serverAddress。
  const serverAddress = validateServerField();
  const usernameValid = validateUsernameField();
  const passwordValid = validatePasswordField();
  if (!serverAddress || !usernameValid || !passwordValid) return;
  emit("submit", {
    serverAddress,
    serverProtocol: setupServerProtocol.value,
    username: setupUsername.value.trim(),
    password: setupPassword.value,
    authMode: authMode.value,
  });
}

function useLocal(): void {
  emit("local", {
    serverAddress: setupServerAddress.value.trim() || DEFAULT_SERVER_ADDRESS,
    serverProtocol: setupServerProtocol.value,
    username: setupUsername.value.trim(),
    password: setupPassword.value,
    authMode: authMode.value,
  });
}

function setAuthMode(mode: AuthMode): void {
  authMode.value = mode;
}

function focusServerInput(): void {
  serverInput.value?.focus();
}

function focusPasswordInput(): void {
  accountPasswordInput.value?.focus();
}

defineExpose({ setFields, setAuthMode, focusServerInput, focusPasswordInput });
</script>

<template>
  <button
    v-if="hasSavedSyncConfig"
    class="icon-button setup-back-button"
    type="button"
    title="返回剪贴板历史"
    aria-label="返回剪贴板历史"
    :disabled="busy"
    @click="emit('close')"
  >
    <ArrowLeft :size="17" aria-hidden="true" />
  </button>

  <section class="setup-content">
    <div class="setup-intro">
      <span class="setup-icon" aria-hidden="true"><Server :size="24" /></span>
      <span class="setup-eyebrow">{{ hasSavedSyncConfig ? "重新登录" : "首次设置" }}</span>
      <h1>{{ authMode === "login" ? "登录同步服务器" : "创建同步账号" }}</h1>
      <p>每个账号拥有独立的剪贴板内容和设备列表。</p>
    </div>

    <form class="setup-form" @submit.prevent="submit">
      <div class="auth-mode-switch" aria-label="账号操作">
        <button type="button" :class="{ active: authMode === 'login' }" :aria-pressed="authMode === 'login'" @click="switchAuthMode('login')">登录</button>
        <button type="button" :class="{ active: authMode === 'register' }" :aria-pressed="authMode === 'register'" @click="switchAuthMode('register')">注册</button>
      </div>

      <div class="server-connection-fields">
        <div class="server-address-field">
          <label for="server-address">服务器地址</label>
          <input
            id="server-address"
            ref="serverInput"
            v-model="setupServerAddress"
            type="text"
            inputmode="text"
            autocomplete="off"
            spellcheck="false"
            placeholder="192.168.1.20:4810"
            :disabled="busy"
            :aria-invalid="Boolean(serverFieldError)"
            :aria-describedby="serverFieldError ? 'server-address-error' : 'server-connection-hint'"
            @blur="validateServerField"
          />
        </div>
        <div class="server-protocol-field">
          <label for="server-protocol">协议</label>
          <select id="server-protocol" v-model="setupServerProtocol" :disabled="busy" aria-describedby="server-connection-hint">
            <option value="http">HTTP</option>
            <option value="https">HTTPS</option>
          </select>
        </div>
      </div>
      <span v-if="serverFieldError" id="server-address-error" class="field-error">{{ serverFieldError }}</span>
      <span v-else id="server-connection-hint" class="field-hint">
        {{ setupServerProtocol === "https"
          ? "HTTPS + WSS：服务端需要配置可信 TLS 证书。"
          : "HTTP + WS 未加密，仅应在受信任的网络中使用。" }}
      </span>

      <label for="account-username">账号</label>
      <input
        id="account-username"
        v-model="setupUsername"
        type="text"
        autocomplete="username"
        spellcheck="false"
        placeholder="请输入账号"
        :disabled="busy"
        :aria-invalid="Boolean(usernameFieldError)"
        :aria-describedby="usernameFieldError ? 'account-username-error' : undefined"
        @blur="validateUsernameField"
      />
      <span v-if="usernameFieldError" id="account-username-error" class="field-error">{{ usernameFieldError }}</span>

      <label for="account-password">密码</label>
      <input
        id="account-password"
        ref="accountPasswordInput"
        v-model="setupPassword"
        type="password"
        :autocomplete="authMode === 'login' ? 'current-password' : 'new-password'"
        placeholder="请输入密码"
        :disabled="busy"
        :aria-invalid="Boolean(passwordFieldError)"
        :aria-describedby="passwordFieldError ? 'account-password-error' : 'account-password-hint'"
        @blur="validatePasswordField"
      />
      <span v-if="passwordFieldError" id="account-password-error" class="field-error">{{ passwordFieldError }}</span>
      <span v-else id="account-password-hint" class="field-hint">密码长度至少 6 位</span>

      <p v-if="error" class="setup-error" role="alert">{{ error }}</p>

      <button class="primary-button" type="submit" :disabled="busy">
        <LoaderCircle v-if="busy" :size="17" class="spin" aria-hidden="true" />
        <ShieldCheck v-else :size="17" aria-hidden="true" />
        {{ busy
          ? (authMode === "login" ? "正在登录…" : "正在创建账号…")
          : (authMode === "login" ? "登录并连接" : "创建账号并连接") }}
      </button>
      <button class="secondary-button" type="button" :disabled="busy" @click="useLocal">
        暂时仅使用本地剪贴板
      </button>
    </form>
  </section>

  <footer class="setup-footer">
    当前设备仅保存登录会话，不保存账号密码
  </footer>
</template>
