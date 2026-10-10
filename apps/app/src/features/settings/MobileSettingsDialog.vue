<script setup lang="ts">
import { computed } from "vue";
import { getActiveConfig } from "../sync/syncSession";
import {
  changePassword, changingPassword, closeSettings, currentPassword, newPassword,
  confirmNewPassword, passwordChangeError, savingSettings, settingsError, signOut,
  switchServer, validateNewPassword, validatePasswordConfirmation,
} from "./useSettings";

defineProps<{ currentUsername: string }>();
const busy = computed(() => savingSettings.value || changingPassword.value);
const serverAddress = getActiveConfig()?.serverAddress ?? "";
</script>

<template>
  <SModal :show="true" title="账号与安全" width="440px"
    :closable="!busy" :mask-closable="!busy" :close-on-esc="!busy" @update:show="closeSettings">
    <div class="mobile-account-settings">
      <SCard title="同步账号">
        <p>当前登录：{{ currentUsername }}</p>
        <p class="mobile-server-address">服务器：{{ serverAddress }}</p>
        <div class="mobile-account-actions">
          <SButton :disabled="busy" @click="switchServer()">切换服务器</SButton>
          <SButton type="error" :disabled="busy" @click="signOut()">退出账号</SButton>
        </div>
      </SCard>
      <SCard title="修改密码">
        <form class="mobile-password-form" @submit.prevent="changePassword">
          <p>修改后，所有设备需要使用新密码重新登录。</p>
          <label for="mobile-current-password">当前密码</label>
          <SInput id="mobile-current-password" v-model:value="currentPassword" type="password"
            autocomplete="current-password" :disabled="busy" />
          <label for="mobile-new-password">新密码</label>
          <SInput id="mobile-new-password" v-model:value="newPassword" type="password"
            autocomplete="new-password" minlength="6" maxlength="128" placeholder="6–128 位"
            :disabled="busy" :invalid="Boolean(passwordChangeError)" @blur="validateNewPassword" />
          <label for="mobile-confirm-password">确认新密码</label>
          <SInput id="mobile-confirm-password" v-model:value="confirmNewPassword" type="password"
            autocomplete="new-password" maxlength="128" :disabled="busy"
            :invalid="Boolean(passwordChangeError)" @blur="validatePasswordConfirmation" />
          <SAlert v-if="passwordChangeError" type="error" role="alert">{{ passwordChangeError }}</SAlert>
          <SButton :loading="changingPassword" :disabled="busy" @click="changePassword">修改密码</SButton>
        </form>
      </SCard>
      <SAlert v-if="settingsError" type="error" role="alert">{{ settingsError }}</SAlert>
    </div>
  </SModal>
</template>

<style scoped>
.mobile-account-settings { display: grid; gap: 16px; }
.mobile-account-settings p { margin: 0 0 12px; line-height: 1.5; }
.mobile-server-address { overflow-wrap: anywhere; }
.mobile-account-actions { display: flex; flex-wrap: wrap; gap: 12px; }
.mobile-password-form { display: grid; gap: 10px; }
.mobile-account-settings :deep(button), .mobile-account-settings :deep(input) { min-height: 48px; }
.mobile-account-settings :deep(input) { font-size: 16px; }
</style>
