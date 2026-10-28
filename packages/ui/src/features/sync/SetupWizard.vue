<script lang="ts">
import type { AuthMode } from "./authApi";

/** 提交给 App.vue 的表单草稿；serverAddress 为规范化后的完整 URL。 */
export type SetupDraft = {
  serverAddress: string;
  username: string;
  password: string;
  authMode: AuthMode;
};
</script>

<script setup lang="ts">
import { appName } from "../../branding";
import { errorMessage } from "../../utils";
import { computed, ref } from "vue";
import { ArrowLeft, Server, ShieldCheck, Trash2 } from "lucide-vue-next";
import { normalizeServerAddress } from "./authApi";
import { DEFAULT_SERVER_ADDRESS } from "../../config";
import type { SyncConfig } from "../../types";

const props = defineProps<{
  hasSavedSyncConfig: boolean;
  busy: boolean;
  error: string;
  verifyingSession: boolean;
  savedAccounts: SyncConfig[];
  canRetrySavedConfig?: boolean;
}>();

const emit = defineEmits<{
  submit: [draft: SetupDraft];
  close: [];
  "reset-error": [];
  "retry-session": [config: SyncConfig];
  "remove-account": [config: SyncConfig];
  "remove-server": [config: SyncConfig];
  "clear-accounts": [];
  "retry-saved-config": [];
}>();

const setupServerAddress = ref(DEFAULT_SERVER_ADDRESS);
const setupUsername = ref("");
const setupPassword = ref("");
const authMode = ref<AuthMode>("login");
const serverFieldError = ref("");
const usernameFieldError = ref("");
const passwordFieldError = ref("");
const serverOptions = computed(() => [...new Set(props.savedAccounts.map((account) => account.serverAddress))]
  .map((value) => ({ label: value, value })));
const currentServerAccounts = computed(() => {
  try {
    const address = normalizeServerAddress(setupServerAddress.value);
    return props.savedAccounts.filter((account) => account.serverAddress === address);
  } catch {
    return [];
  }
});
const accountOptions = computed(() => currentServerAccounts.value.map((account) => ({
  label: `${account.username} · ${account.sessionToken ? '可直接登录' : '需输入密码'}`,
  value: account.username,
})));
const selectedAccount = computed(() => currentServerAccounts.value.find((account) => account.username === setupUsername.value.trim()));
const matchingSavedSession = computed(() => {
  if (authMode.value !== "login") return undefined;
  return selectedAccount.value?.sessionToken ? selectedAccount.value : undefined;
});
const matchesSavedSession = computed(() => Boolean(matchingSavedSession.value));
const sessionVerificationError = computed(() => props.error.startsWith("服务器尚未支持登录状态验证")
  ? "服务器暂不支持自动登录，请输入密码登录，或更新服务器。"
  : "");
const canUseSavedSession = computed(() => matchesSavedSession.value && !setupPassword.value);
const passwordHint = computed(() => matchesSavedSession.value
  ? "已保存登录状态，可直接登录；输入密码可重新登录。"
  : "密码长度至少 6 位");
// SInput 根元素是 span 外壳，ref 拿不到内部 input：容器 ref + querySelector 聚焦。
const serverInput = ref<{ $el?: Element }>();
const accountPasswordInput = ref<{ $el?: Element }>();

function setFields(config?: SyncConfig): void {
  setupServerAddress.value = config?.serverAddress || DEFAULT_SERVER_ADDRESS;
  setupUsername.value = config?.username || "";
  setupPassword.value = "";
  authMode.value = "login";
  serverFieldError.value = "";
  usernameFieldError.value = "";
  passwordFieldError.value = "";
}

function resetCredentials(): void {
  setupPassword.value = "";
  usernameFieldError.value = "";
  passwordFieldError.value = "";
  emit("reset-error");
}

function fillServerAccount(): void {
  setupUsername.value = authMode.value === "login" ? currentServerAccounts.value[0]?.username ?? "" : "";
  resetCredentials();
}

function selectServer(value: string): void {
  const account = props.savedAccounts.find((account) => account.serverAddress === value);
  if (!account) return;
  setupServerAddress.value = value;
  serverFieldError.value = "";
  fillServerAccount();
}

function updateServerAddress(value: string): void {
  setupServerAddress.value = value;
  serverFieldError.value = "";
  fillServerAccount();
}

function selectAccount(username: string): void {
  setupUsername.value = username;
  resetCredentials();
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
  if (canUseSavedSession.value) {
    passwordFieldError.value = "";
    return true;
  }
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
  if (props.busy) return;
  // 校验并规范化完整 URL 后再提交。
  const serverAddress = validateServerField();
  const usernameValid = validateUsernameField();
  const passwordValid = validatePasswordField();
  if (!serverAddress || !usernameValid || !passwordValid) return;
  if (canUseSavedSession.value && matchingSavedSession.value) {
    emit("retry-session", matchingSavedSession.value);
    return;
  }
  emit("submit", {
    serverAddress,
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
      <div class="setup-brand">
        <span class="setup-icon" aria-hidden="true"><Server :size="20" /></span>
        <span class="setup-eyebrow">{{ verifyingSession ? "正在恢复登录" : appName }}</span>
      </div>
      <h1>{{ authMode === "login" ? "登录同步服务器" : "创建同步账号" }}</h1>
      <p>每个账号拥有独立的剪贴板内容和设备列表。</p>
    </div>

    <SForm class="setup-form" @submit.prevent="submit">
      <div class="auth-mode-switch" aria-label="账号操作">
        <button type="button" :disabled="busy" :class="{ active: authMode === 'login' }" :aria-pressed="authMode === 'login'" @click="switchAuthMode('login')">登录</button>
        <button type="button" :disabled="busy" :class="{ active: authMode === 'register' }" :aria-pressed="authMode === 'register'" @click="switchAuthMode('register')">注册</button>
      </div>

      <div class="server-connection-fields">
        <SFormItem label="服务器地址" :error="serverFieldError">
          <div class="login-choice-field">
            <SInput
              id="server-address"
              ref="serverInput"
              :value="setupServerAddress"
              type="text"
              inputmode="url"
              autocomplete="off"
              spellcheck="false"
              placeholder="https://example.com/cliproam"
              :disabled="busy"
              :invalid="Boolean(serverFieldError)"
              @update:value="updateServerAddress"
              @blur="validateServerField"
            >
              <template v-if="serverOptions.length" #suffix>
                <SSelect class="login-choice-menu" :value="setupServerAddress" :options="serverOptions" :menu-width="320" :disabled="busy" aria-label="选择已保存的服务器" title="选择已保存的服务器" @select="selectServer" />
              </template>
            </SInput>
            <SIconButton v-if="currentServerAccounts.length" :size="36" :disabled="busy" title="删除此服务器的全部登录记录" aria-label="删除此服务器的全部登录记录" @click="emit('remove-server', currentServerAccounts[0]!)">
              <Trash2 :size="16" aria-hidden="true" />
            </SIconButton>
          </div>
        </SFormItem>
      </div>
      <p class="connection-hint">填写完整的 http:// 或 https:// 地址，支持子路径，例如 https://example.com/cliproam。</p>

      <SFormItem label="账号" :error="usernameFieldError">
        <div class="login-choice-field">
          <SInput
            id="account-username"
            :value="setupUsername"
            type="text"
            autocomplete="username"
            spellcheck="false"
            placeholder="选择或输入账号"
            :disabled="busy"
            :invalid="Boolean(usernameFieldError)"
            @update:value="selectAccount"
            @blur="validateUsernameField"
          >
            <template v-if="authMode === 'login'" #suffix>
              <SSelect class="login-choice-menu" :value="setupUsername" :options="accountOptions.length ? accountOptions : [{ label: '此服务器暂无记录，请手动输入账号', value: '', disabled: true }]" :menu-width="280" :disabled="busy" aria-label="选择此服务器的已存账号" title="选择已保存的账号" @select="selectAccount" />
            </template>
          </SInput>
          <SIconButton v-if="authMode === 'login' && selectedAccount" :size="36" :disabled="busy" title="删除此账号的登录记录" aria-label="删除此账号的登录记录" @click="emit('remove-account', selectedAccount)">
            <Trash2 :size="16" aria-hidden="true" />
          </SIconButton>
        </div>
      </SFormItem>

      <SFormItem label="密码" :hint="passwordHint" :error="passwordFieldError">
        <SInput
          id="account-password"
          ref="accountPasswordInput"
          v-model:value="setupPassword"
          type="password"
          :autocomplete="authMode === 'login' ? 'current-password' : 'new-password'"
          :placeholder="matchesSavedSession ? '留空使用已保存的登录状态' : '请输入密码'"
          :disabled="busy"
          :invalid="Boolean(passwordFieldError)"
          @blur="validatePasswordField"
        />
      </SFormItem>

      <SAlert v-if="error && !sessionVerificationError" type="error">{{ error }}</SAlert>

      <SButton v-if="canRetrySavedConfig" :disabled="busy" block @click="emit('retry-saved-config')">
        重试读取登录状态
      </SButton>

      <SButton type="primary" :loading="busy" block @click="submit">
        <ShieldCheck v-if="!busy" :size="16" aria-hidden="true" />
        {{ busy
          ? (verifyingSession ? "正在验证登录状态…" : authMode === "login" ? "正在登录…" : "正在创建账号…")
          : (authMode === "login" ? "登录" : "创建账号") }}
      </SButton>
      <SButton v-if="authMode === 'login' && savedAccounts.length" type="text" :disabled="busy" block @click="emit('clear-accounts')">清空所有登录记录</SButton>
    </SForm>
  </section>

  <footer class="setup-footer">
    当前设备仅保存登录会话，不保存账号密码
  </footer>
</template>
