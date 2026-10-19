<script setup lang="ts">
import { appName } from "@cliproam/ui/branding";
import { Bug, RefreshCw } from "lucide-vue-next";
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { errorMessage } from "@cliproam/ui/errors";
import SettingsDialog from "@cliproam/ui/settings-dialog";
import { useRustSettings } from "./useRustSettings";
import "./styles.css";

defineProps<{ currentUsername: string }>();
const {
    autostartEnabled, autostartLoading, autostartLoaded, autostartError,
    isMobile, platformCapabilities, autoUploadLimitMb, autoReceiveClipboard,
    watchClipboard, excludePatternsInput, recordingQuickPasteShortcut, busy,
    savingSettings, autoUploadSelectOptions, recordQuickPasteShortcut, selectQuickPasteShortcut,
    quickPasteShortcut, quickPasteShortcutRefreshing, quickPasteShortcutStatus, displayShortcut,
    updaterSupported, updateStatus, updateStatusText, checkForUpdate, openAppDataDirectory, saveSettings,
} = useRustSettings();

const debugToolsSupported = ref(false);
const openingDebugTools = ref(false);
const debugToolsError = ref("");
onMounted(async () => {
  debugToolsSupported.value = await invoke<boolean>("native_debug_tools_supported").catch(() => false);
});
async function openDebugTools(): Promise<void> {
  if (openingDebugTools.value) return;
  openingDebugTools.value = true;
  debugToolsError.value = "";
  try { await invoke("native_open_devtools"); }
  catch (error) { debugToolsError.value = `调试工具打开失败：${errorMessage(error)}`; }
  finally { openingDebugTools.value = false; }
}
</script>
<template>
  <SettingsDialog :current-username="currentUsername" :save="saveSettings" :save-pages="['general', 'shortcuts']">
    <template #general>
      <SCard v-if="!isMobile" title="启动设置">
        <label class="setting-switch" for="autostart">
          <span class="setting-switch-copy">
            <strong>开机启动</strong>
            <small>登录系统后自动启动 {{ appName }}，点击保存后生效。</small>
          </span>
          <SSwitch
            id="autostart"
            v-model:value="autostartEnabled"
            :disabled="busy || !autostartLoaded"
          />
        </label>
        <p v-if="autostartLoading" class="settings-section-desc" role="status">正在读取开机启动状态…</p>
        <p v-if="autostartError" class="settings-section-desc" role="alert">{{ autostartError }}</p>
      </SCard>
      <SCard title="剪贴板漫游">
        <p class="settings-section-desc">{{ isMobile
          ? "开启监听后自动记录本机剪贴板文本，也可点击“读取当前剪贴板”导入；其他设备的内容同步到历史，点按文本即可复制。"
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
        <p class="settings-section-desc">配置当前设备普通文件的自动上传大小上限。截图等剪贴板图片按服务器独立上限自动上传。</p>
        <label for="auto-upload-limit">自动上传普通文件</label>
        <SSelect
          id="auto-upload-limit"
          v-model:value="autoUploadLimitMb"
          :options="autoUploadSelectOptions"
          :disabled="savingSettings"
        />
        <span class="field-hint">关闭或超出此上限的普通文件不会自动上传，粘贴时需要源设备在线。此设置不影响剪贴板图片。</span>
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
    </template>
    <template #platform-tabs>
      <STabPane v-if="platformCapabilities.globalShortcut" name="shortcuts" tab="快捷键">
        <div class="settings-page">
          <header class="settings-page-header">
            <h3>快捷键</h3>
            <p>配置当前设备的全局快捷操作，不会同步到其他设备。</p>
          </header>
          <SCard title="快捷粘贴">
            <p class="settings-section-desc">在其他应用中按下快捷键，打开 {{ appName }} 快捷粘贴窗口。</p>
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
    </template>
    <template #about>
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
      <SCard v-if="debugToolsSupported" title="调试">
        <p class="settings-section-desc">查看页面、控制台和本地存储。</p>
        <SButton :loading="openingDebugTools" @click="openDebugTools">
          <Bug v-if="!openingDebugTools" :size="16" aria-hidden="true" />
          打开调试工具
        </SButton>
        <p v-if="debugToolsError" class="settings-section-desc" role="alert">{{ debugToolsError }}</p>
      </SCard>
    </template>
  </SettingsDialog>
</template>
