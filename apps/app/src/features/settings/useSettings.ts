import { nextTick, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { runningInTauri, usePlatform } from "../../composables/usePlatform";
import {
  quickPasteShortcut,
  quickPasteShortcutStatus,
  resetQuickPasteShortcutDraft,
  saveQuickPasteShortcut,
} from "../quick-paste/quickPasteShortcut";
import { changeAccountPassword } from "../sync/syncClient";
import type { SettingsPage, SyncConfig } from "../../types";

/**
 * 设置弹窗的模块级单例（对齐 usePlatform 风格）。弹窗状态被侧边栏、
 * HistoryView 的 open-settings 事件和全局键盘（Esc）三方共享，所以提升到
 * 模块层；触达同步引擎的部分通过 App.vue 注入的 bridge 完成。
 */
export type SettingsBridge = {
  getActiveConfig(): SyncConfig | undefined;
  setActiveConfig(config: SyncConfig): void;
  getUsername(): string;
  setUsername(name: string): void;
  persistSyncConfig(config: SyncConfig): Promise<void>;
  startSync(config: SyncConfig): Promise<void>;
  /** 断开当前同步客户端；参数为断开后的 syncEnabled 值（改密后为 true，退出账号为 false）。 */
  disconnect(syncEnabledAfter: boolean): void;
  uploadNowEligibleEntries(bytes: number): void;
  openSetup(o: { config?: SyncConfig; message?: string; focus?: "server" | "password" }): void;
  focusSearch(): void;
};

let bridge: SettingsBridge | undefined;

export function initSettings(settingsBridge: SettingsBridge): void {
  bridge = settingsBridge;
}

function requireBridge(): SettingsBridge {
  if (!bridge) throw new Error("Settings bridge 尚未初始化（应先在 App.vue 中调用 initSettings）");
  return bridge;
}

const settingsVisible = ref(false);
const settingsPage = ref<SettingsPage>("general");
const autoUploadLimitMb = ref(10);
const autoReceiveClipboard = ref(true);
const savingSettings = ref(false);
const recordingQuickPasteShortcut = ref(false);
const changingPassword = ref(false);
const settingsError = ref("");
const passwordChangeError = ref("");
const currentPassword = ref("");
const newPassword = ref("");
const confirmNewPassword = ref("");

function openSettings(): void {
  const activeConfig = requireBridge().getActiveConfig();
  if (!activeConfig) return;
  autoUploadLimitMb.value = activeConfig.autoUploadLimitMb;
  autoReceiveClipboard.value = activeConfig.autoReceiveClipboard;
  resetQuickPasteShortcutDraft();
  recordingQuickPasteShortcut.value = false;
  settingsPage.value = "general";
  clearPasswordChangeFields();
  settingsError.value = "";
  settingsVisible.value = true;
}

function selectSettingsPage(page: SettingsPage): void {
  settingsPage.value = page;
  recordingQuickPasteShortcut.value = false;
  settingsError.value = "";
}

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

function recordQuickPasteShortcut(event: KeyboardEvent): void {
  if (!recordingQuickPasteShortcut.value) return;
  event.preventDefault();
  event.stopPropagation();
  if (event.code === "Escape") {
    recordingQuickPasteShortcut.value = false;
    return;
  }
  const token = shortcutKeyToken(event);
  if (!token) return;

  const modifiers: string[] = [];
  if (event.ctrlKey || event.metaKey) modifiers.push("CommandOrControl");
  if (event.shiftKey) modifiers.push("Shift");
  if (event.altKey) modifiers.push("Alt");
  if (!modifiers.length && !/^F\d{1,2}$/.test(token)) {
    quickPasteShortcutStatus.value = {
      state: "error",
      message: "请至少按住 Ctrl、Alt 或 Command，再按一个主键",
    };
    return;
  }
  quickPasteShortcut.value = [...modifiers, token].join("+");
  quickPasteShortcutStatus.value = { state: "idle", message: "点击保存后生效" };
  recordingQuickPasteShortcut.value = false;
}

function selectQuickPasteShortcut(shortcut: string): void {
  quickPasteShortcut.value = shortcut;
  quickPasteShortcutStatus.value = { state: "idle", message: "点击保存后生效" };
}

function closeSettings(): void {
  if (savingSettings.value || changingPassword.value) return;
  settingsVisible.value = false;
  settingsError.value = "";
  clearPasswordChangeFields();
  void nextTick(() => { void requireBridge().focusSearch(); });
}

function clearPasswordChangeFields(): void {
  currentPassword.value = "";
  newPassword.value = "";
  confirmNewPassword.value = "";
  passwordChangeError.value = "";
}

function validateNewPassword(): boolean {
  const length = newPassword.value.length;
  passwordChangeError.value = length >= 6 && length <= 128 ? "" : "新密码长度需为 6-128 位";
  return !passwordChangeError.value;
}

function validatePasswordConfirmation(): boolean {
  passwordChangeError.value = newPassword.value === confirmNewPassword.value ? "" : "两次输入的新密码不一致";
  return !passwordChangeError.value;
}

async function openAppDataDirectory(): Promise<void> {
  if (!runningInTauri) return;
  settingsError.value = "";
  try {
    await invoke("open_app_data_dir");
  } catch (error) {
    settingsError.value = `无法打开应用数据目录：${error instanceof Error ? error.message : String(error)}`;
  }
}

async function saveSettings(): Promise<void> {
  const activeConfig = requireBridge().getActiveConfig();
  if (!activeConfig || savingSettings.value || changingPassword.value) return;
  const { platformCapabilities } = usePlatform();
  savingSettings.value = true;
  settingsError.value = "";
  const previousAutoUploadLimitMb = activeConfig.autoUploadLimitMb;
  const config = {
    ...activeConfig,
    autoUploadLimitMb: autoUploadLimitMb.value,
    autoReceiveClipboard: autoReceiveClipboard.value,
  };
  try {
    if (
      runningInTauri
      && platformCapabilities.value.globalShortcut
      && settingsPage.value === "shortcuts"
      && !(await saveQuickPasteShortcut())
    ) {
      settingsError.value = quickPasteShortcutStatus.value.message;
      return;
    }
    await requireBridge().persistSyncConfig(config);
    requireBridge().setActiveConfig(config);
    if (config.enabled && config.username && config.sessionToken) await requireBridge().startSync(config);
    if (config.autoUploadLimitMb > previousAutoUploadLimitMb) {
      requireBridge().uploadNowEligibleEntries(config.autoUploadLimitMb * 1024 * 1024);
    }
    settingsVisible.value = false;
    await nextTick();
    await requireBridge().focusSearch();
  } catch (error) {
    settingsError.value = `无法保存设置：${error instanceof Error ? error.message : String(error)}`;
  } finally {
    savingSettings.value = false;
  }
}

async function changePassword(): Promise<void> {
  const activeConfig = requireBridge().getActiveConfig();
  if (!activeConfig || !requireBridge().getUsername() || changingPassword.value || savingSettings.value) return;
  if (currentPassword.value.length < 6 || currentPassword.value.length > 128) {
    passwordChangeError.value = "请输入当前密码";
    return;
  }
  if (!validateNewPassword() || !validatePasswordConfirmation()) {
    return;
  }
  if (newPassword.value === currentPassword.value) {
    passwordChangeError.value = "新密码不能与当前密码相同";
    return;
  }

  changingPassword.value = true;
  settingsError.value = "";
  try {
    await changeAccountPassword(
      activeConfig.serverAddress,
      activeConfig.serverProtocol,
      activeConfig.sessionToken,
      currentPassword.value,
      newPassword.value,
    );
    const config: SyncConfig = {
      ...activeConfig,
      enabled: true,
      sessionToken: "",
    };
    await requireBridge().persistSyncConfig(config);
    requireBridge().setActiveConfig(config);
    requireBridge().setUsername(config.username);
    requireBridge().disconnect(true);
    settingsVisible.value = false;
    clearPasswordChangeFields();
    requireBridge().openSetup({
      config,
      message: "密码已修改，请使用新密码重新登录",
      focus: "password",
    });
  } catch (error) {
    settingsError.value = `修改密码失败：${error instanceof Error ? error.message : String(error)}`;
  } finally {
    changingPassword.value = false;
  }
}

async function signOut(openLogin: boolean): Promise<void> {
  const activeConfig = requireBridge().getActiveConfig();
  if (!activeConfig || savingSettings.value || changingPassword.value) return;
  savingSettings.value = true;
  settingsError.value = "";
  const config: SyncConfig = {
    ...activeConfig,
    enabled: false,
    username: "",
    sessionToken: "",
  };
  try {
    await requireBridge().persistSyncConfig(config);
    requireBridge().setActiveConfig(config);
    requireBridge().setUsername("");
    requireBridge().disconnect(false);
    settingsVisible.value = false;
    if (openLogin) {
      requireBridge().openSetup({ config, focus: "server" });
    } else {
      await nextTick();
      await requireBridge().focusSearch();
    }
  } catch (error) {
    settingsError.value = `无法退出账号：${error instanceof Error ? error.message : String(error)}`;
  } finally {
    savingSettings.value = false;
  }
}

export {
  settingsVisible,
  settingsPage,
  autoUploadLimitMb,
  autoReceiveClipboard,
  savingSettings,
  recordingQuickPasteShortcut,
  changingPassword,
  settingsError,
  passwordChangeError,
  currentPassword,
  newPassword,
  confirmNewPassword,
  openSettings,
  selectSettingsPage,
  recordQuickPasteShortcut,
  selectQuickPasteShortcut,
  closeSettings,
  validateNewPassword,
  validatePasswordConfirmation,
  openAppDataDirectory,
  saveSettings,
  changePassword,
  signOut,
};
