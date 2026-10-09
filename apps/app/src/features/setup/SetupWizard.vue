<script lang="ts">
import type { AuthMode, ServerProtocol } from "../sync/syncSetup";

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
import { computed, ref } from "vue";
import { ArrowLeft, Server, ShieldCheck } from "lucide-vue-next";
import { normalizeServerAddress } from "../sync/syncSetup";
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
// SInput 根元素是 span 外壳，ref 拿不到内部 input：容器 ref + querySelector 聚焦。
const serverInput = ref<{ $el?: Element }>();
const accountPasswordInput = ref<{ $el?: Element }>();

const protocolHint = computed(() =>
  setupServerProtocol.value === "https"
    ? "HTTPS + WSS：服务端需要配置可信 TLS 证书。"
    : "HTTP + WS 未加密，仅应在受信任的网络中使用。",
);

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

function setAuthMode(mode: AuthMode): void {
  authMode.value = mode;
}

function focusServerInput(): void {
  serverInput.value?.$el?.querySelector<HTMLInputElement>("input")?.focus();
}

function focusPasswordInput(): void {
  accountPasswordInput.value?.$el?.querySelector<HTMLInputElement>("input")?.focus();
}

defineExpose({ setFields, setAuthMode, focusServerInput, focusPasswordInput });
</script>

<template>
  <SIconButton
    v-if="hasSavedSyncConfig"
    class="setup-back-button"
    :size="30"
    title="返回剪贴板历史"
    aria-label="返回剪贴板历史"
    :disabled="busy"
    @click="emit('close')"
  >
    <ArrowLeft :size="17" aria-hidden="true" />
  </SIconButton>

  <section class="setup-content">
    <div class="setup-intro">
      <span class="setup-icon" aria-hidden="true"><Server :size="24" /></span>
      <span class="setup-eyebrow">{{ hasSavedSyncConfig ? "重新登录" : "首次设置" }}</span>
      <h1>{{ authMode === "login" ? "登录同步服务器" : "创建同步账号" }}</h1>
      <p>每个账号拥有独立的剪贴板内容和设备列表。</p>
    </div>

    <SForm class="setup-form" @submit.prevent="submit">
      <div class="auth-mode-switch" aria-label="账号操作">
        <button type="button" :class="{ active: authMode === 'login' }" :aria-pressed="authMode === 'login'" @click="switchAuthMode('login')">登录</button>
        <button type="button" :class="{ active: authMode === 'register' }" :aria-pressed="authMode === 'register'" @click="switchAuthMode('register')">注册</button>
      </div>

      <div class="server-connection-fields">
        <SFormItem label="服务器地址" :error="serverFieldError">
          <SInput
            id="server-address"
            ref="serverInput"
            v-model:value="setupServerAddress"
            type="text"
            inputmode="text"
            autocomplete="off"
            spellcheck="false"
            placeholder="192.168.1.20:4810"
            :disabled="busy"
            :invalid="Boolean(serverFieldError)"
            @blur="validateServerField"
          />
        </SFormItem>
        <SFormItem label="协议" :hint="protocolHint">
          <SSelect
            id="server-protocol"
            v-model:value="setupServerProtocol"
            :options="[{ label: 'HTTP', value: 'http' }, { label: 'HTTPS', value: 'https' }]"
            :disabled="busy"
          />
        </SFormItem>
      </div>

      <SFormItem label="账号" :error="usernameFieldError">
        <SInput
          id="account-username"
          v-model:value="setupUsername"
          type="text"
          autocomplete="username"
          spellcheck="false"
          placeholder="请输入账号"
          :disabled="busy"
          :invalid="Boolean(usernameFieldError)"
          @blur="validateUsernameField"
        />
      </SFormItem>

      <SFormItem label="密码" hint="密码长度至少 6 位" :error="passwordFieldError">
        <SInput
          id="account-password"
          ref="accountPasswordInput"
          v-model:value="setupPassword"
          type="password"
          :autocomplete="authMode === 'login' ? 'current-password' : 'new-password'"
          placeholder="请输入密码"
          :disabled="busy"
          :invalid="Boolean(passwordFieldError)"
          @blur="validatePasswordField"
        />
      </SFormItem>

      <SAlert v-if="error" type="error">{{ error }}</SAlert>

      <SButton type="primary" :loading="busy" block @click="submit">
        <ShieldCheck v-if="!busy" :size="16" aria-hidden="true" />
        {{ busy
          ? (authMode === "login" ? "正在登录…" : "正在创建账号…")
          : (authMode === "login" ? "登录并连接" : "创建账号并连接") }}
      </SButton>
    </SForm>
  </section>

  <footer class="setup-footer">
    当前设备仅保存登录会话，不保存账号密码
  </footer>
</template>
