<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { ArrowLeft, ChevronRight, Info, Server, ShieldCheck, Smartphone, UserRound, Workflow, X } from "lucide-vue-next";
import { appName } from "../../branding";
import { clientSessionManager } from "../sync/clientSessionManager";
import AboutPanel from "./AboutPanel.vue";
import ServerLimitsCard from "./ServerLimitsCard.vue";
import {
  changePassword, changingPassword, closeSettings, currentPassword, newPassword,
  confirmNewPassword, passwordChangeError, savingSettings, settingsError, signOut,
  validateNewPassword, validatePasswordConfirmation, deviceAliasInput, systemDeviceName, saveSettings,
} from "./useSettings";

export interface MobileSettingsSection { id: string; title: string; description: string; }
const props = withDefaults(defineProps<{
  currentUsername: string;
  sections?: MobileSettingsSection[];
}>(), { sections: () => [] });
const busy = computed(() => savingSettings.value || changingPassword.value);
const serverAddress = computed(() => clientSessionManager.getActiveConfig()?.serverAddress ?? "");
const page = ref("");
const scrollArea = ref<HTMLElement>();
const pageHeading = ref<HTMLElement>();
let overviewScroll = 0;
const groups = computed(() => [
  { title: "本机与同步", items: [
    { id: "device", title: "本机设备", description: deviceAliasInput.value || systemDeviceName.value || "设置设备在同步中的名称", icon: Smartphone },
    { id: "server", title: "服务器限制", description: "文件大小、历史条数与便签额度", icon: Server },
  ] },
  ...(props.sections.length ? [{ title: "功能与集成", items: props.sections.map(section => ({ ...section, icon: Workflow })) }] : []),
  { title: "帮助与支持", items: [
    { id: "about", title: `关于 ${appName.value}`, description: "版本信息、使用指南与问题反馈", icon: Info },
  ] },
]);
const pageTitle = computed(() => page.value === "account" ? "账号与安全"
  : groups.value.flatMap(group => group.items).find(item => item.id === page.value)?.title ?? "设置");

async function navigate(id: string): Promise<void> {
  if (busy.value) return;
  const previous = page.value;
  if (!previous) overviewScroll = scrollArea.value?.scrollTop ?? 0;
  if (previous === "account") {
    currentPassword.value = newPassword.value = confirmNewPassword.value = "";
    passwordChangeError.value = "";
  }
  settingsError.value = "";
  page.value = id;
  await nextTick();
  if (scrollArea.value) scrollArea.value.scrollTop = id ? 0 : overviewScroll;
  if (id) pageHeading.value?.focus({ preventScroll: true });
  else scrollArea.value?.querySelectorAll<HTMLButtonElement>("[data-section]")
    .forEach(button => { if (button.dataset.section === previous) button.focus({ preventScroll: true }); });
}
watch(pageTitle, title => { if (page.value && title === "设置") void navigate(""); });
</script>

<template>
  <SModal class="mobile-settings-modal" :show="true" :title="pageTitle" width="640px" fill-height
    :body-scrollable="false" :closable="false" :mask-closable="!busy" :close-on-esc="!busy" @update:show="closeSettings">
    <header class="settings-navigation">
      <button v-if="page" type="button" class="settings-navigation-button" :disabled="busy" aria-label="返回设置" @click="navigate('')">
        <ArrowLeft :size="22" aria-hidden="true" />
      </button>
      <span v-else aria-hidden="true" />
      <h2>{{ pageTitle }}</h2>
      <button type="button" class="settings-navigation-button" :disabled="busy" aria-label="关闭设置" @click="closeSettings">
        <X :size="20" aria-hidden="true" />
      </button>
    </header>
    <div ref="scrollArea" class="mobile-settings-scroll">
      <div v-if="!page" class="settings-overview">
        <button type="button" class="settings-account" data-section="account" @click="navigate('account')">
          <span class="settings-avatar"><UserRound :size="24" aria-hidden="true" /></span>
          <span class="settings-row-copy">
            <strong>{{ currentUsername || '未登录账号' }}</strong>
            <span>账号与安全<ChevronRight :size="12" aria-hidden="true" /></span>
          </span>
          <ShieldCheck :size="20" class="settings-account-shield" aria-hidden="true" />
        </button>
        <section v-for="group in groups" :key="group.title" class="settings-group" :aria-label="group.title">
          <h3>{{ group.title }}</h3>
          <div class="settings-list">
            <button v-for="item in group.items" :key="item.id" type="button" class="settings-row"
              :data-section="item.id" @click="navigate(item.id)">
              <span class="settings-row-icon" :class="`settings-icon-${item.id}`"><component :is="item.icon" :size="20" aria-hidden="true" /></span>
              <span class="settings-row-copy"><strong>{{ item.title }}</strong><span>{{ item.description }}</span></span>
              <ChevronRight :size="18" class="settings-chevron" aria-hidden="true" />
            </button>
          </div>
        </section>
      </div>
      <section v-else ref="pageHeading" class="settings-detail" tabindex="-1" :aria-label="pageTitle">
        <template v-if="page === 'account'">
          <SCard title="同步账号">
            <div class="settings-account-label"><ShieldCheck :size="20" aria-hidden="true" /><strong>{{ currentUsername || '未登录账号' }}</strong></div>
            <p v-if="serverAddress" class="settings-server-address">{{ serverAddress }}</p>
          </SCard>
          <SCard v-if="currentUsername" title="修改密码">
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
          <SButton type="error" :disabled="busy || !currentUsername" @click="signOut()">退出账号</SButton>
        </template>
        <form v-else-if="page === 'device'" class="settings-detail-form" @submit.prevent="saveSettings()">
          <SCard title="设备名称">
            <label for="mobile-device-alias">在设备列表与历史来源中显示的名称</label>
            <SInput id="mobile-device-alias" v-model:value="deviceAliasInput" maxlength="80"
              :placeholder="systemDeviceName || '系统机器名'" :disabled="busy" />
            <p>留空使用系统机器名，保存后立即更新。</p>
          </SCard>
          <SButton type="primary" :loading="savingSettings" :disabled="busy" @click="saveSettings()">保存设置</SButton>
        </form>
        <ServerLimitsCard v-else-if="page === 'server' && currentUsername" />
        <AboutPanel v-else-if="page === 'about'" />
        <slot v-else :section="page" />
      </section>
      <SAlert v-if="settingsError" class="mobile-settings-error" type="error" role="alert">{{ settingsError }}</SAlert>
    </div>
  </SModal>
</template>

<style scoped>
:global(.mobile-settings-modal .s-modal-box) { padding: env(safe-area-inset-top, 0px) env(safe-area-inset-right, 0px) 0 env(safe-area-inset-left, 0px); box-sizing: border-box; background: var(--color-background); }
:global(.mobile-settings-modal .s-modal-header) { display: none; }
:global(.mobile-settings-modal .s-modal-body) { flex: 1; padding: 0; overflow: hidden; }
.settings-navigation { display: grid; grid-template-columns: 48px minmax(0, 1fr) 48px; align-items: center; flex: none; min-height: 60px; padding: 4px 12px; border-bottom: 1px solid var(--sui-border-subtle); }
.settings-navigation h2 { margin: 0; overflow: hidden; color: var(--sui-fg); font-size: 17px; font-weight: 600; line-height: 1.5; text-align: center; text-overflow: ellipsis; white-space: nowrap; }
.settings-navigation-button { display: grid; place-items: center; width: 48px; height: 48px; padding: 0; border: 0; border-radius: 12px; background: transparent; color: var(--sui-fg-secondary); cursor: pointer; touch-action: manipulation; }
.settings-navigation-button:disabled { opacity: .45; cursor: default; }
.mobile-settings-scroll { flex: 1; min-height: 0; overflow-y: auto; overscroll-behavior: contain; padding: 20px 20px max(32px, env(safe-area-inset-bottom, 0px)); color: var(--sui-fg); }
.settings-overview, .settings-detail, .settings-detail-form { display: grid; align-content: start; gap: 24px; min-width: 0; max-width: 560px; margin: 0 auto; }
.settings-account, .settings-row { display: flex; align-items: center; gap: 12px; width: 100%; padding: 14px 16px; border: 0; color: var(--sui-fg); background: var(--sui-bg); font: inherit; text-align: left; cursor: pointer; touch-action: manipulation; transition: background 150ms ease; }
.settings-account { min-height: 96px; padding: 20px; border: 1px solid var(--sui-border-subtle); border-radius: 16px; }
.settings-avatar { display: grid; place-items: center; width: 48px; height: 48px; flex-shrink: 0; border: 1px solid var(--sui-success-soft); border-radius: 16px; color: var(--sui-on-success-soft); background: var(--sui-success-soft); }
.settings-row-copy { display: grid; gap: 3px; flex: 1; min-width: 0; overflow-wrap: anywhere; }
.settings-row-copy strong { font-size: 15px; font-weight: 500; line-height: 1.5; }
.settings-row-copy > span { color: var(--sui-fg-muted); font-size: 12px; line-height: 1.5; }
.settings-account .settings-row-copy strong { font-size: 20px; font-weight: 600; }
.settings-account .settings-row-copy > span { display: flex; align-items: center; gap: 4px; font-size: 13px; }
.settings-account-shield { flex: none; color: var(--sui-on-success-soft); }
.settings-group h3 { margin: 0 0 8px 16px; color: var(--sui-fg-muted); font-size: 12px; font-weight: 500; line-height: 1.5; }
.settings-list { overflow: hidden; border: 1px solid var(--sui-border-subtle); border-radius: 14px; }
.settings-row { position: relative; min-height: 72px; }
.settings-row + .settings-row::before { position: absolute; content: ''; inset: 0 16px auto 64px; height: 1px; background: var(--sui-border-subtle); }
.settings-row-icon { display: grid; place-items: center; width: 36px; height: 36px; flex-shrink: 0; border-radius: 10px; color: var(--sui-on-info-soft); background: var(--sui-info-soft); }
.settings-icon-device { color: var(--sui-on-success-soft); background: var(--sui-success-soft); }
.settings-icon-about { color: var(--sui-fg-secondary); background: var(--sui-bg-active); }
.settings-chevron { flex-shrink: 0; color: var(--sui-fg-muted); }
.settings-account:active, .settings-row:active, .settings-navigation-button:active { background: var(--sui-bg-active); }
.settings-account:focus-visible, .settings-row:focus-visible, .settings-navigation-button:focus-visible { outline: 2px solid var(--sui-primary); outline-offset: -3px; }
.settings-detail, .settings-detail-form { gap: 16px; }
.settings-detail:focus { outline: none; }
.settings-account-label { display: flex; align-items: center; gap: 10px; font-size: 16px; }
.settings-account-label svg { flex: none; color: var(--sui-on-success-soft); }
.settings-server-address { overflow-wrap: anywhere; }
.settings-detail p { margin: 0; color: var(--sui-fg-muted); font-size: 13px; line-height: 1.7; }
.settings-detail label { color: var(--sui-fg-secondary); font-size: 14px; line-height: 1.5; }
.mobile-password-form { display: grid; gap: 12px; }
.settings-detail :deep(button), .settings-detail :deep(input) { min-height: 48px; }
.settings-detail :deep(input) { font-size: 16px; }
.settings-detail :deep(.s-card) { min-width: 0; border-color: var(--sui-border-subtle); border-radius: 14px; box-shadow: none; }
.settings-detail :deep(.s-card-header) { min-height: auto; padding: 16px 16px 0; border-bottom: 0; }
.settings-detail :deep(.s-card-title) { color: var(--sui-fg); font-size: 15px; font-weight: 600; }
.settings-detail :deep(.s-card-body) { display: grid; gap: 12px; padding: 16px; }
.settings-detail :deep(.s-button) { border-radius: 10px; }
.mobile-settings-error { max-width: 560px; margin: 16px auto 0; }
@media (hover: hover) { .settings-account:hover, .settings-row:hover, .settings-navigation-button:hover { background: var(--sui-bg-hover); } }
@media (max-width: 720px) {
  :global(.mobile-settings-modal) { padding: 0; }
  :global(.mobile-settings-modal .s-modal-box.fill-height) { width: 100% !important; height: 100dvh; max-height: 100dvh; border: 0; border-radius: 0; }
  .mobile-settings-scroll { padding-left: 16px; padding-right: 16px; }
}
@media (prefers-reduced-motion: reduce) { .settings-account, .settings-row { transition: none; } }
</style>
