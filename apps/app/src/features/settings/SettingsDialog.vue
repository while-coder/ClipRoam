<script setup lang="ts">
import {
  Check,
  Clipboard,
  Cloud,
  FolderOpen,
  KeyRound,
  LoaderCircle,
  RefreshCw,
  ShieldCheck,
  X,
} from "lucide-vue-next";
import { runningInTauri, usePlatform } from "../../composables/usePlatform";
import {
  displayShortcut,
  quickPasteShortcut,
  quickPasteShortcutRefreshing,
  quickPasteShortcutStatus,
} from "../quick-paste/quickPasteShortcut";
import { useUpdater } from "./useUpdater";
import {
  autoReceiveClipboard,
  autoUploadLimitMb,
  changePassword,
  changingPassword,
  closeSettings,
  currentPassword,
  newPassword,
  confirmNewPassword,
  openAppDataDirectory,
  passwordChangeError,
  recordQuickPasteShortcut,
  recordingQuickPasteShortcut,
  saveSettings,
  savingSettings,
  selectQuickPasteShortcut,
  selectSettingsPage,
  settingsError,
  settingsPage,
  signOut,
  validateNewPassword,
  validatePasswordConfirmation,
} from "./useSettings";

defineProps<{
  currentUsername: string;
  syncEnabled: boolean;
}>();

const { platformCapabilities, isMobile } = usePlatform();

const {
  appVersion,
  updaterSupported,
  updateStatus,
  updateStatusText,
  checkForUpdate,
} = useUpdater();

async function checkAppUpdate(): Promise<void> {
  await checkForUpdate({ silent: false });
}
</script>

<template>
    <div class="settings-backdrop" @mousedown.self="closeSettings">
      <section class="settings-dialog" role="dialog" aria-modal="true" aria-labelledby="settings-heading">
        <header class="settings-dialog-header">
          <div>
            <span>设置</span>
            <h2 id="settings-heading">本机偏好</h2>
          </div>
          <button class="icon-button" type="button" title="关闭设置" aria-label="关闭设置" :disabled="savingSettings || changingPassword" @click="closeSettings">
            <X :size="17" />
          </button>
        </header>

        <div class="settings-layout">
          <nav class="settings-nav" aria-label="设置分类" role="tablist">
            <button :class="{ active: settingsPage === 'general' }" type="button" role="tab" aria-controls="settings-general-panel" :aria-selected="settingsPage === 'general'" @click="selectSettingsPage('general')">通用</button>
            <button v-if="runningInTauri && platformCapabilities.globalShortcut" :class="{ active: settingsPage === 'shortcuts' }" type="button" role="tab" aria-controls="settings-shortcuts-panel" :aria-selected="settingsPage === 'shortcuts'" @click="selectSettingsPage('shortcuts')">快捷键</button>
            <button :class="{ active: settingsPage === 'account' }" type="button" role="tab" aria-controls="settings-account-panel" :aria-selected="settingsPage === 'account'" @click="selectSettingsPage('account')">账号与安全</button>
            <button :class="{ active: settingsPage === 'data' }" type="button" role="tab" aria-controls="settings-data-panel" :aria-selected="settingsPage === 'data'" @click="selectSettingsPage('data')">应用数据</button>
            <button :class="{ active: settingsPage === 'about' }" type="button" role="tab" aria-controls="settings-about-panel" :aria-selected="settingsPage === 'about'" @click="selectSettingsPage('about')">关于</button>
          </nav>

          <form class="settings-form" @submit.prevent="saveSettings">
            <section v-if="settingsPage === 'general'" id="settings-general-panel" class="settings-page" role="tabpanel" aria-labelledby="general-settings-heading">
              <header class="settings-page-header">
                <h3 id="general-settings-heading">通用</h3>
                <p>配置当前设备的剪贴板漫游和文件同步行为。</p>
              </header>
              <section class="settings-section" aria-labelledby="roaming-settings-heading">
                <div class="settings-section-heading">
                  <span class="settings-icon" aria-hidden="true"><Clipboard :size="18" /></span>
                  <div>
                    <h4 id="roaming-settings-heading">剪贴板漫游</h4>
                    <p>其他在线设备复制内容后，直接更新本机系统剪贴板。</p>
                  </div>
                </div>
                <label class="setting-switch" for="auto-receive-clipboard">
                  <span class="setting-switch-copy">
                    <strong>自动接收剪贴板</strong>
                    <small>{{ isMobile
                      ? "移动端前台支持文本；图片和文件同步到历史后可手动下载。"
                      : "支持文本、富文本和图片；文件与文件夹只同步到历史，需手动选择粘贴。" }}</small>
                  </span>
                  <input id="auto-receive-clipboard" v-model="autoReceiveClipboard" type="checkbox" role="switch" :disabled="savingSettings" />
                  <span class="setting-switch-track" aria-hidden="true"></span>
                </label>
              </section>
              <section class="settings-section" aria-labelledby="upload-settings-heading">
                <div class="settings-section-heading">
                  <span class="settings-icon" aria-hidden="true"><FolderOpen :size="18" /></span>
                  <div>
                    <h4 id="upload-settings-heading">文件同步</h4>
                    <p>配置当前设备自动上传到同步服务的文件大小上限。</p>
                  </div>
                </div>
                <label for="auto-upload-limit">自动上传文件</label>
                <select id="auto-upload-limit" v-model.number="autoUploadLimitMb" :disabled="savingSettings">
                  <option :value="0">关闭自动上传</option>
                  <option :value="1">小于 1 MB</option>
                  <option :value="2">小于 2 MB</option>
                  <option :value="5">小于 5 MB</option>
                  <option :value="10">小于 10 MB</option>
                  <option :value="20">小于 20 MB</option>
                  <option :value="50">小于 50 MB</option>
                  <option :value="100">小于 100 MB</option>
                </select>
                <span class="field-hint">超过上限的文件不会自动上传，粘贴时需要源设备在线。</span>
              </section>
            </section>

            <section v-else-if="settingsPage === 'shortcuts'" id="settings-shortcuts-panel" class="settings-page" role="tabpanel" aria-labelledby="shortcuts-page-heading">
              <header class="settings-page-header">
                <h3 id="shortcuts-page-heading">快捷键</h3>
                <p>配置当前设备的全局快捷操作，不会同步到其他设备。</p>
              </header>
              <section class="settings-section" aria-labelledby="quick-paste-shortcut-heading">
                <div class="settings-section-heading">
                  <span class="settings-icon" aria-hidden="true"><KeyRound :size="18" /></span>
                  <div>
                    <h4 id="quick-paste-shortcut-heading">快捷粘贴</h4>
                    <p>在其他应用中按下快捷键，打开 ClipRoam 快捷粘贴窗口。</p>
                  </div>
                </div>
                <div class="shortcut-setting-row">
                  <div>
                    <strong>全局快捷键</strong>
                    <small>点击右侧按钮，然后按下新的组合键；Esc 取消录制。</small>
                  </div>
                  <button
                    class="shortcut-recorder"
                    :class="{ recording: recordingQuickPasteShortcut }"
                    type="button"
                    :disabled="savingSettings || quickPasteShortcutRefreshing"
                    :aria-label="recordingQuickPasteShortcut ? '正在录制快捷粘贴快捷键' : `当前快捷键 ${displayShortcut(quickPasteShortcut)}`"
                    @click="recordingQuickPasteShortcut = true"
                    @blur="recordingQuickPasteShortcut = false"
                    @keydown="recordQuickPasteShortcut"
                  >
                    {{ recordingQuickPasteShortcut ? "按下组合键…" : displayShortcut(quickPasteShortcut) }}
                  </button>
                </div>
                <div class="shortcut-presets" aria-label="快捷键预设">
                  <span>预设</span>
                  <button
                    v-for="preset in ['CommandOrControl+Shift+V', 'CommandOrControl+Alt+V', 'CommandOrControl+Shift+Space']"
                    :key="preset"
                    type="button"
                    :class="{ active: quickPasteShortcut === preset }"
                    :disabled="savingSettings || quickPasteShortcutRefreshing"
                    @click="selectQuickPasteShortcut(preset)"
                  >
                    {{ displayShortcut(preset) }}
                  </button>
                </div>
                <p v-if="quickPasteShortcutStatus.message" class="shortcut-status" :class="quickPasteShortcutStatus.state" :role="quickPasteShortcutStatus.state === 'error' ? 'alert' : 'status'" aria-live="polite">
                  {{ quickPasteShortcutStatus.message }}
                </p>
              </section>
            </section>

            <section v-else-if="settingsPage === 'account'" id="settings-account-panel" class="settings-page" role="tabpanel" aria-labelledby="account-page-heading">
              <header class="settings-page-header">
                <h3 id="account-page-heading">账号与安全</h3>
                <p>管理同步账号和登录安全。</p>
              </header>
              <section class="settings-section" aria-labelledby="account-settings-heading">
                <div class="settings-section-heading">
                  <span class="settings-icon" aria-hidden="true"><Cloud :size="18" /></span>
                  <div>
                    <h4 id="account-settings-heading">账号</h4>
                    <p>{{ currentUsername ? `当前登录：${currentUsername}` : "当前未登录同步账号" }}</p>
                  </div>
                </div>
                <div class="account-actions">
                  <button class="secondary-button" type="button" :disabled="savingSettings || changingPassword" @click="signOut(true)">切换账号</button>
                  <button class="danger-button" type="button" :disabled="savingSettings || changingPassword || !currentUsername" @click="signOut(false)">退出账号</button>
                </div>
              </section>

              <section v-if="syncEnabled && currentUsername" class="settings-section" aria-labelledby="password-settings-heading">
                <div class="settings-section-heading">
                  <span class="settings-icon" aria-hidden="true"><ShieldCheck :size="18" /></span>
                  <div>
                    <h4 id="password-settings-heading">修改密码</h4>
                    <p>修改后，所有设备需要使用新密码重新登录。</p>
                  </div>
                </div>
                <div class="password-change-fields">
                  <label for="current-password">当前密码</label>
                  <input id="current-password" v-model="currentPassword" type="password" autocomplete="current-password" :disabled="savingSettings || changingPassword" />
                  <label for="new-password">新密码</label>
                  <input id="new-password" v-model="newPassword" type="password" autocomplete="new-password" minlength="6" maxlength="128" placeholder="至少 6 位" :aria-invalid="Boolean(passwordChangeError)" :aria-describedby="passwordChangeError ? 'password-change-error' : 'password-change-hint'" :disabled="savingSettings || changingPassword" @blur="validateNewPassword" />
                  <label for="confirm-new-password">确认新密码</label>
                  <input id="confirm-new-password" v-model="confirmNewPassword" type="password" autocomplete="new-password" minlength="6" maxlength="128" :aria-invalid="Boolean(passwordChangeError)" :aria-describedby="passwordChangeError ? 'password-change-error' : 'password-change-hint'" :disabled="savingSettings || changingPassword" @blur="validatePasswordConfirmation" />
                </div>
                <span v-if="passwordChangeError" id="password-change-error" class="field-error" role="alert">{{ passwordChangeError }}</span>
                <span v-else id="password-change-hint" class="field-hint">新密码长度为 6-128 位。</span>
                <button class="secondary-button" type="button" :disabled="savingSettings || changingPassword" @click="changePassword">
                  <LoaderCircle v-if="changingPassword" :size="17" class="spin" aria-hidden="true" />
                  {{ changingPassword ? "正在修改…" : "修改密码" }}
                </button>
              </section>
            </section>

            <section v-else-if="settingsPage === 'data'" id="settings-data-panel" class="settings-page" role="tabpanel" aria-labelledby="data-page-heading">
              <header class="settings-page-header">
                <h3 id="data-page-heading">应用数据</h3>
                <p>查看当前设备保存的历史和配置文件。</p>
              </header>
              <section class="settings-section" aria-labelledby="data-settings-heading">
                <div class="settings-section-heading">
                  <span class="settings-icon" aria-hidden="true"><FolderOpen :size="18" /></span>
                  <div>
                    <h4 id="data-settings-heading">本地数据目录</h4>
                    <p>{{ isMobile
                      ? "移动端数据保存在系统应用沙箱中，卸载应用时会一并移除。"
                      : "包含本地剪贴板历史、同步配置和已保存的文件。" }}</p>
                  </div>
                </div>
                <button v-if="platformCapabilities.openDataDirectory" class="secondary-button" type="button" :disabled="savingSettings || changingPassword" @click="openAppDataDirectory">打开应用数据</button>
              </section>
            </section>

            <section v-else id="settings-about-panel" class="settings-page" role="tabpanel" aria-labelledby="about-page-heading">
              <header class="settings-page-header">
                <h3 id="about-page-heading">关于</h3>
                <p>查看应用版本和更新状态。</p>
              </header>

              <section class="settings-section about-product" aria-labelledby="about-product-heading">
                <img class="about-product-mark" src="/cliproam-icon.png" alt="" />
                <div class="about-product-copy">
                  <h4 id="about-product-heading">ClipRoam</h4>
                  <p>让剪贴板内容在你的设备之间安全漫游。</p>
                </div>
                <span class="about-version">v{{ appVersion || "…" }}</span>
              </section>

              <section class="settings-section about-update" aria-labelledby="update-settings-heading">
                <div class="settings-section-heading">
                  <span class="settings-icon" aria-hidden="true"><RefreshCw :size="18" /></span>
                  <div>
                    <h4 id="update-settings-heading">应用更新</h4>
                    <p>检查 GitHub Release 中是否有可用的新版本。</p>
                  </div>
                </div>
                <p class="about-update-status" :class="{ 'update-error': updateStatus === 'error' }" :role="updateStatus === 'error' ? 'alert' : 'status'" aria-live="polite">
                  {{ updateStatusText }}
                </p>
                <button
                  class="secondary-button about-update-button"
                  type="button"
                  :disabled="!updaterSupported || updateStatus === 'checking' || updateStatus === 'downloading'"
                  @click="checkAppUpdate"
                >
                  <LoaderCircle v-if="updateStatus === 'checking'" :size="17" class="spin" aria-hidden="true" />
                  <RefreshCw v-else :size="17" aria-hidden="true" />
                  {{ !updaterSupported
                    ? "当前平台不支持"
                    : updateStatus === "checking"
                      ? "检查中…"
                      : updateStatus === "downloading"
                        ? "下载中…"
                        : "检查更新" }}
                </button>
              </section>
            </section>

            <p v-if="settingsError" class="setup-error" role="alert">{{ settingsError }}</p>

            <footer v-if="settingsPage === 'general' || settingsPage === 'shortcuts'" class="settings-actions">
              <button class="primary-button" type="submit" :disabled="savingSettings || changingPassword">
                <LoaderCircle v-if="savingSettings" :size="17" class="spin" aria-hidden="true" />
                <Check v-else :size="17" aria-hidden="true" />
                {{ savingSettings ? "正在保存…" : "保存设置" }}
              </button>
            </footer>
          </form>
        </div>
      </section>
    </div>
</template>
