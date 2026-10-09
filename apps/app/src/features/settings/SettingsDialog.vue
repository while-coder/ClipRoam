<script setup lang="ts">
import { Check, RefreshCw } from "lucide-vue-next";
import { computed } from "vue";
import { usePlatform } from "../../composables/usePlatform";
import type { SettingsPage } from "../../types";
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
  excludePatternsInput,
  serverMaxFileMb,
  deviceAliasInput,
  systemDeviceName,
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
  useVirtualFiles,
  validateNewPassword,
  validatePasswordConfirmation,
  watchClipboard,
} from "./useSettings";

defineProps<{
  currentUsername: string;
}>();

const { platformCapabilities, isMobile } = usePlatform();

/** 保存或改密进行中：禁止关闭弹窗、禁止重复提交。 */
const busy = computed(() => savingSettings.value || changingPassword.value);

/**
 * 自动上传档位：0 = 关闭。刻度按服务器单文件存储上限动态生成——
 * 上限很小的服务器也至少能选到「小于上限本身」，而不是只剩关闭。
 * 当前已存档位始终并入选项，避免 select 值不在列表里时显示错位。
 */
const autoUploadLimitOptions = computed(() => {
  const max = serverMaxFileMb.value;
  const standard = [10, 25, 50, 75, 100, 150, 200, 300, 500, 1000]
    .filter((limit) => limit <= max);
  const values = new Set<number>([0, ...standard, max, autoUploadLimitMb.value]);
  return [...values].filter((limit) => limit === 0 || limit <= max).sort((a, b) => a - b);
});

/** SSelect 需要 {label, value} 项；value 保持数字，v-model:value 回写后无需转换。 */
const autoUploadSelectOptions = computed(() =>
  autoUploadLimitOptions.value.map((limit) => ({
    label: limit === 0 ? "关闭自动上传" : `小于 ${limit} MB`,
    value: limit,
  })),
);

const {
  appVersion,
  updaterSupported,
  updateStatus,
  updateStatusText,
  checkForUpdate,
} = useUpdater();
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
      <form class="settings-form" @submit.prevent="saveSettings">
        <STabs
          placement="left"
          :value="settingsPage"
          @update:value="selectSettingsPage($event as SettingsPage)"
        >
          <STabPane name="general" tab="通用">
            <div class="settings-page">
              <header class="settings-page-header">
                <h3>通用</h3>
                <p>配置当前设备的剪贴板漫游和文件同步行为。</p>
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
              <SCard title="剪贴板漫游">
                <p class="settings-section-desc">{{ isMobile
                  ? "点击“读取当前剪贴板”导入文本；其他设备的内容同步到历史，点按文本即可复制。"
                  : "本机复制的内容自动记入历史并同步；其他设备复制的内容可直接更新本机剪贴板。" }}</p>
                <label v-if="platformCapabilities.clipboardMonitoring" class="setting-switch" for="watch-clipboard">
                  <span class="setting-switch-copy">
                    <strong>监听剪贴板</strong>
                    <small>关闭后本机复制不再自动记入历史，只接收其他设备同步过来的内容。</small>
                  </span>
                  <SSwitch
                    id="watch-clipboard"
                    v-model:value="watchClipboard"
                    :disabled="savingSettings"
                  />
                </label>
                <label v-if="!isMobile" class="setting-switch" for="auto-receive-clipboard">
                  <span class="setting-switch-copy">
                    <strong>自动接收剪贴板</strong>
                    <small>支持文本、富文本和图片；文件与文件夹只同步到历史，需手动选择粘贴。</small>
                  </span>
                  <SSwitch
                    id="auto-receive-clipboard"
                    v-model:value="autoReceiveClipboard"
                    :disabled="savingSettings"
                  />
                </label>
              </SCard>
              <SCard title="文件同步">
                <p class="settings-section-desc">配置当前设备自动上传到同步服务的文件大小上限。</p>
                <label for="auto-upload-limit">自动上传文件</label>
                <SSelect
                  id="auto-upload-limit"
                  v-model:value="autoUploadLimitMb"
                  :options="autoUploadSelectOptions"
                  :disabled="savingSettings"
                />
                <span class="field-hint">超过上限的文件不会自动上传，粘贴时需要源设备在线。</span>
                <label v-if="platformCapabilities.virtualFilePaste" class="setting-switch" for="use-virtual-files">
                  <span class="setting-switch-copy">
                    <strong>虚拟文件粘贴</strong>
                    <small>默认关闭：文件先下载到本机再粘贴，与 Mac/Linux 行为一致；开启后直接以虚拟文件粘贴。</small>
                  </span>
                  <SSwitch
                    id="use-virtual-files"
                    v-model:value="useVirtualFiles"
                    :disabled="savingSettings"
                  />
                </label>
              </SCard>
              <SCard title="复制过滤">
                <p class="settings-section-desc">复制文件/文件夹时按名称跳过匹配的内容，不进入历史与同步。</p>
                <label for="exclude-patterns">过滤名称</label>
                <STextarea
                  id="exclude-patterns"
                  v-model:value="excludePatternsInput"
                  :rows="4"
                  placeholder="node_modules&#10;.git&#10;*.log"
                  spellcheck="false"
                  :disabled="savingSettings"
                />
                <span class="field-hint">每行一个名称，支持 * 和 ? 通配；只匹配名称本身，不区分大小写。</span>
              </SCard>
            </div>
          </STabPane>

          <STabPane v-if="platformCapabilities.globalShortcut" name="shortcuts" tab="快捷键">
            <div class="settings-page">
              <header class="settings-page-header">
                <h3>快捷键</h3>
                <p>配置当前设备的全局快捷操作，不会同步到其他设备。</p>
              </header>
              <SCard title="快捷粘贴">
                <p class="settings-section-desc">在其他应用中按下快捷键，打开 ClipRoam 快捷粘贴窗口。</p>
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
              </SCard>
            </div>
          </STabPane>

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

          <STabPane name="data" tab="应用数据">
            <div class="settings-page">
              <header class="settings-page-header">
                <h3>应用数据</h3>
                <p>查看当前设备保存的历史和配置文件。</p>
              </header>
              <SCard title="本地数据目录">
                <p class="settings-section-desc">{{ isMobile
                  ? "移动端数据保存在系统应用沙箱中，卸载应用时会一并移除。"
                  : "包含本地剪贴板历史、同步配置和已保存的文件。" }}</p>
                <SButton v-if="platformCapabilities.openDataDirectory" :disabled="busy" @click="openAppDataDirectory">打开应用数据</SButton>
              </SCard>
            </div>
          </STabPane>

          <STabPane name="about" tab="关于">
            <div class="settings-page">
              <header class="settings-page-header">
                <h3>关于</h3>
                <p>查看应用版本和更新状态。</p>
              </header>

              <SCard class="about-product">
                <img class="about-product-mark" src="/cliproam-icon.png" alt="" />
                <div class="about-product-copy">
                  <h4>ClipRoam</h4>
                  <p>让剪贴板内容在你的设备之间安全漫游。</p>
                </div>
                <span class="about-version">v{{ appVersion || "…" }}</span>
              </SCard>

              <SCard class="about-update" title="应用更新">
                <template #header-extra>
                  <SButton
                    class="about-update-button"
                    :loading="updateStatus === 'checking'"
                    :disabled="!updaterSupported || updateStatus === 'checking' || updateStatus === 'downloading'"
                    @click="checkForUpdate({ silent: false })"
                  >
                    <RefreshCw v-if="updateStatus !== 'checking'" :size="16" aria-hidden="true" />
                    {{ !updaterSupported
                      ? "当前平台不支持"
                      : updateStatus === "checking"
                        ? "检查中…"
                        : updateStatus === "downloading"
                          ? "下载中…"
                          : "检查更新" }}
                  </SButton>
                </template>
                <p class="settings-section-desc">检查 GitHub Release 中是否有可用的新版本。</p>
                <p class="about-update-status" :class="{ 'update-error': updateStatus === 'error' }" :role="updateStatus === 'error' ? 'alert' : 'status'" aria-live="polite">
                  {{ updateStatusText }}
                </p>
              </SCard>
            </div>
          </STabPane>
        </STabs>

        <SAlert v-if="settingsError" type="error">{{ settingsError }}</SAlert>

        <footer v-if="settingsPage === 'general' || settingsPage === 'shortcuts'" class="settings-actions">
          <SButton type="primary" :loading="savingSettings" :disabled="changingPassword" @click="saveSettings">
            <Check v-if="!savingSettings" :size="16" aria-hidden="true" />
            {{ savingSettings ? "正在保存…" : "保存设置" }}
          </SButton>
        </footer>
      </form>
    </SModal>
</template>
