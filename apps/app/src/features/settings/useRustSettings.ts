import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useTauriUpdater } from "../updater/useTauriUpdater";
import { savingSettings, changingPassword, settingsVisible, settingsPage, settingsError, saveSettings as saveSharedSettings, getSettingsPreferences } from "@cliproam/ui/settings";
import { native } from "@cliproam/ui/native";
import { usePlatform } from "@cliproam/ui/platform";
import { errorMessage } from "@cliproam/ui/errors";
import { displayShortcut, quickPasteShortcut, quickPasteShortcutRefreshing, quickPasteShortcutStatus, resetQuickPasteShortcutDraft, saveQuickPasteShortcut } from "../quick-paste/quickPasteShortcut";
import { bossKeyEnabled, bossKeyShortcut, bossKeyRefreshing, bossKeyStatus, resetBossKeyDraft, saveBossKey } from "./bossKey";

export function useRustSettings() {
  const { isMobile, platformCapabilities } = usePlatform();
  const preferences = getSettingsPreferences();
  const autoUploadLimitMb = ref(Math.min(preferences.autoUploadLimitMb, preferences.serverMaxFileMb));
  const autoReceiveClipboardText = ref(preferences.autoReceiveClipboardText);
  const autoReceiveClipboardImage = ref(preferences.autoReceiveClipboardImage);
  const watchClipboardText = ref(preferences.watchClipboardText);
  const watchClipboardImage = ref(preferences.watchClipboardImage);
  const watchClipboardFiles = ref(preferences.watchClipboardFiles);
  const excludePatternsInput = ref(preferences.excludePatterns.join("\n"));
  const serverMaxFileMb = computed(() => getSettingsPreferences().serverMaxFileMb);
  watch(serverMaxFileMb, (max) => { autoUploadLimitMb.value = Math.min(autoUploadLimitMb.value, max); });
  const recordingQuickPasteShortcut = ref(false);
  const recordingBossKey = ref(false);
  const busy = computed(() => savingSettings.value || changingPassword.value);
  const autostartEnabled = ref(false);
  const autostartLoading = ref(false);
  const autostartLoaded = ref(false);
  const autostartError = ref("");
  let savedAutostart: boolean | undefined;
  watch(settingsVisible, async (visible) => {
    if (!visible || isMobile.value) return;
    autostartLoading.value = true;
    autostartLoaded.value = false;
    autostartError.value = "";
    savedAutostart = undefined;
    try {
      savedAutostart = await invoke<boolean>("autostart_is_enabled");
      autostartEnabled.value = savedAutostart;
      autostartLoaded.value = true;
    } catch (error) {
      autostartError.value = `无法读取开机启动状态：${errorMessage(error)}`;
    } finally {
      autostartLoading.value = false;
    }
  }, { immediate: true });
  resetQuickPasteShortcutDraft();
  resetBossKeyDraft();
  watch(settingsPage, () => {
    recordingQuickPasteShortcut.value = false;
    recordingBossKey.value = false;
  });
  watch(bossKeyEnabled, () => { recordingBossKey.value = false; });
  const { updaterSupported, updateStatus, updateStatusText, checkForUpdate, requestInstallPermission } = useTauriUpdater();
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

  function shortcutKeyToken(event: KeyboardEvent): string {
    const { code } = event;
    if (code.startsWith("Key")) return code.slice(3);
    if (code.startsWith("Digit")) return code.slice(5);
    if (/^F\d{1,2}$/.test(code)) return code;
    const tokens: Record<string, string> = {
      Space: "Space",
      Enter: "Enter",
      Tab: "Tab",
      Backquote: "`",
      Minus: "-",
      Equal: "=",
      ArrowUp: "Up",
      ArrowDown: "Down",
      ArrowLeft: "Left",
      ArrowRight: "Right",
    };
    return tokens[code] ?? "";
  }

  function recordShortcut(event: KeyboardEvent, bossKey = false): void {
    const recording = bossKey ? recordingBossKey : recordingQuickPasteShortcut;
    const shortcut = bossKey ? bossKeyShortcut : quickPasteShortcut;
    const status = bossKey ? bossKeyStatus : quickPasteShortcutStatus;
    if (!recording.value) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.code === "Escape") {
      recording.value = false;
      return;
    }
    const token = shortcutKeyToken(event);
    if (!token) return;

    const modifiers: string[] = [];
    if (event.ctrlKey || event.metaKey) modifiers.push("CommandOrControl");
    if (event.shiftKey) modifiers.push("Shift");
    if (event.altKey) modifiers.push("Alt");
    if (!modifiers.length && !/^F\d{1,2}$/.test(token)) {
      status.value = {
        state: "error",
        message: "请至少按住 Ctrl、Alt 或 Command，再按一个主键",
      };
      return;
    }
    shortcut.value = [...modifiers, token].join("+");
    status.value = { state: "idle", message: "点击保存后生效" };
    recording.value = false;
  }

  function recordQuickPasteShortcut(event: KeyboardEvent): void { recordShortcut(event); }
  function recordBossKey(event: KeyboardEvent): void { recordShortcut(event, true); }

  function selectQuickPasteShortcut(shortcut: string): void {
    quickPasteShortcut.value = shortcut;
    quickPasteShortcutStatus.value = { state: "idle", message: "点击保存后生效" };
  }


  async function openAppDataDirectory(): Promise<void> {
    settingsError.value = "";
    try { await native.openAppDataDir(); }
    catch (error) { settingsError.value = `无法打开应用数据目录：${errorMessage(error)}`; }
  }
  async function saveSettings(): Promise<void> {
    await saveSharedSettings({
      preferences: {
        ...getSettingsPreferences(),
        autoUploadLimitMb: autoUploadLimitMb.value,
        autoReceiveClipboardText: autoReceiveClipboardText.value,
        autoReceiveClipboardImage: autoReceiveClipboardImage.value,
        watchClipboardText: watchClipboardText.value,
        watchClipboardImage: watchClipboardImage.value,
        watchClipboardFiles: watchClipboardFiles.value,
        excludePatterns: [...new Set(excludePatternsInput.value.split("\n").map(value => value.trim()).filter(Boolean))],
      },
      beforeSave: async () => {
        if (!isMobile.value && settingsPage.value === "general") {
          if (autostartLoading.value) throw new Error("正在读取开机启动状态，请稍后重试");
          if (savedAutostart !== undefined && autostartEnabled.value !== savedAutostart) {
            try {
              await invoke("autostart_set_enabled", { enabled: autostartEnabled.value });
              savedAutostart = autostartEnabled.value;
              autostartError.value = "";
            } catch (error) {
              autostartError.value = `无法设置开机启动：${errorMessage(error)}`;
              throw new Error(autostartError.value);
            }
          }
        }
        if (platformCapabilities.value.globalShortcut && settingsPage.value === "shortcuts" && !await saveQuickPasteShortcut()) {
          throw new Error(quickPasteShortcutStatus.value.message);
        }
        if (platformCapabilities.value.globalShortcut && settingsPage.value === "shortcuts" && !await saveBossKey()) {
          throw new Error(bossKeyStatus.value.message);
        }
      },
    });
  }
  return {
    autostartEnabled, autostartLoading, autostartLoaded, autostartError,
    isMobile, platformCapabilities, autoUploadLimitMb, autoReceiveClipboardText, autoReceiveClipboardImage,
    watchClipboardText, watchClipboardImage, watchClipboardFiles, excludePatternsInput, recordingQuickPasteShortcut, busy,
    savingSettings, autoUploadSelectOptions, recordQuickPasteShortcut, selectQuickPasteShortcut,
    quickPasteShortcut, quickPasteShortcutRefreshing, quickPasteShortcutStatus, displayShortcut,
    bossKeyEnabled, bossKeyShortcut, bossKeyRefreshing, bossKeyStatus, recordingBossKey, recordBossKey,
    updaterSupported, updateStatus, updateStatusText, checkForUpdate, requestInstallPermission, openAppDataDirectory, saveSettings,
  };
}
