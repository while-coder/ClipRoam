<script setup lang="ts">
import { Check } from "lucide-vue-next";
import { computed } from "vue";
import type { SettingsPage } from "../../types";
import { useAppVersion } from "./useAppVersion";
import productIcon from "../../assets/cliproam-icon.png";
import {
  deviceAliasInput,
  systemDeviceName,
  changePassword,
  changingPassword,
  closeSettings,
  currentPassword,
  newPassword,
  confirmNewPassword,
  passwordChangeError,
  saveSettings,
  savingSettings,
  selectSettingsPage,
  settingsError,
  settingsPage,
  signOut,
  validateNewPassword,
  validatePasswordConfirmation,
} from "./useSettings";

const props = withDefaults(defineProps<{
  currentUsername: string;
  save?: () => Promise<void>;
  savePages?: string[];
}>(), { savePages: () => ["general"] });
const submitSettings = () => props.save ? props.save() : saveSettings();


/** 保存或改密进行中：禁止关闭弹窗、禁止重复提交。 */
const busy = computed(() => savingSettings.value || changingPassword.value);

const { appVersion } = useAppVersion();
</script>

<template>
    <SModal
      :show="true"
      title="本机偏好"
      width="820px"
      fill-height
      :body-scrollable="false"
      :closable="!busy"
      :mask-closable="!busy"
      :close-on-esc="!busy"
      @update:show="closeSettings"
    >
      <form class="settings-form" @submit.prevent="submitSettings">
        <STabs
          placement="left"
          :value="settingsPage"
          @update:value="selectSettingsPage($event as SettingsPage)"
        >
          <STabPane name="general" tab="通用">
            <div class="settings-page">
              <header class="settings-page-header">
                <h3>通用</h3>
                <p>设置当前设备的显示名称。</p>
              </header>
              <SCard title="本机设备">
                <p class="settings-section-desc">设备列表与历史来源中显示的名称。</p>
                <SInput
                  id="device-alias"
                  v-model:value="deviceAliasInput"
                  type="text"
                  maxlength="80"
                  :placeholder="systemDeviceName || '系统机器名'"
                  spellcheck="false"
                  :disabled="savingSettings"
                />
                <span class="field-hint">留空使用系统机器名；保存后立即上报新名称。</span>
              </SCard>
              <slot name="general" />
            </div>
          </STabPane>

          <slot name="platform-tabs" />

          <STabPane name="account" tab="账号与安全">
            <div class="settings-page">
              <header class="settings-page-header">
                <h3>账号与安全</h3>
                <p>管理同步账号和登录安全。</p>
              </header>
              <SCard title="账号">
                <p class="settings-section-desc">{{ currentUsername ? `当前登录：${currentUsername}` : "当前未登录同步账号" }}</p>
                <div class="account-actions">
                  <SButton type="error" :disabled="busy || !currentUsername" @click="signOut()">退出账号</SButton>
                </div>
              </SCard>

              <SCard v-if="currentUsername" title="修改密码">
                <p class="settings-section-desc">修改后，所有设备需要使用新密码重新登录。</p>
                <div class="password-change-fields">
                  <label for="current-password">当前密码</label>
                  <SInput
                    id="current-password"
                    v-model:value="currentPassword"
                    type="password"
                    autocomplete="current-password"
                    :disabled="busy"
                  />
                  <label for="new-password">新密码</label>
                  <SInput
                    id="new-password"
                    v-model:value="newPassword"
                    type="password"
                    autocomplete="new-password"
                    minlength="6"
                    maxlength="128"
                    placeholder="至少 6 位"
                    :invalid="Boolean(passwordChangeError)"
                    :aria-describedby="passwordChangeError ? 'password-change-error' : 'password-change-hint'"
                    :disabled="busy"
                    @blur="validateNewPassword"
                  />
                  <label for="confirm-new-password">确认新密码</label>
                  <SInput
                    id="confirm-new-password"
                    v-model:value="confirmNewPassword"
                    type="password"
                    autocomplete="new-password"
                    minlength="6"
                    maxlength="128"
                    :invalid="Boolean(passwordChangeError)"
                    :aria-describedby="passwordChangeError ? 'password-change-error' : 'password-change-hint'"
                    :disabled="busy"
                    @blur="validatePasswordConfirmation"
                  />
                </div>
                <span v-if="passwordChangeError" id="password-change-error" class="field-error" role="alert">{{ passwordChangeError }}</span>
                <span v-else id="password-change-hint" class="field-hint">新密码长度为 6-128 位。</span>
                <SButton :loading="changingPassword" :disabled="savingSettings" @click="changePassword">
                  {{ changingPassword ? "正在修改…" : "修改密码" }}
                </SButton>
              </SCard>
            </div>
          </STabPane>

          <STabPane name="about" tab="关于">
            <div class="settings-page">
              <header class="settings-page-header">
                <h3>关于</h3>
                <p>查看应用信息和版本。</p>
              </header>

              <SCard class="about-product">
                <img class="about-product-mark" :src="productIcon" alt="" />
                <div class="about-product-copy">
                  <h4>ClipRoam</h4>
                  <p>让剪贴板内容在你的设备之间安全漫游。</p>
                </div>
                <span class="about-version">v{{ appVersion || "…" }}</span>
              </SCard>

              <slot name="about" />
            </div>
          </STabPane>
        </STabs>

        <SAlert v-if="settingsError" type="error">{{ settingsError }}</SAlert>

        <footer v-if="props.savePages.includes(settingsPage)" class="settings-actions">
          <SButton type="primary" :loading="savingSettings" :disabled="changingPassword" @click="submitSettings">
            <Check v-if="!savingSettings" :size="16" aria-hidden="true" />
            {{ savingSettings ? "正在保存…" : "保存设置" }}
          </SButton>
        </footer>
      </form>
    </SModal>
</template>
