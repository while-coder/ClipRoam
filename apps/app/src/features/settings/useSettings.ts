import { nextTick, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { usePlatform } from "../../composables/usePlatform";
import { errorMessage } from "../../utils/error";
import {
  quickPasteShortcut,
  quickPasteShortcutStatus,
  resetQuickPasteShortcutDraft,
  saveQuickPasteShortcut,
} from "../quick-paste/quickPasteShortcut";
import { changeAccountPassword, pushDeviceInfo } from "../sync/syncSetup";
import { DEFAULT_AUTO_RECEIVE_CLIPBOARD, DEFAULT_AUTO_UPLOAD_LIMIT_MB, DEFAULT_SERVER_MAX_FILE_MB } from "../sync/syncDefaults";
import { getDevice, getDeviceIdentity } from "../../utils/device";
import type { AccountPreferences, SettingsPage, SyncConfig } from "../../types";

/**
 * 设置弹窗的模块级单例（对齐 usePlatform 风格）。弹窗状态被侧边栏、
 * HistoryView 的 open-settings 事件和全局键盘（Esc）三方共享，所以提升到
 * 模块层；触达同步引擎的部分通过 App.vue 注入的 bridge 完成。
 */
export type SettingsBridge = {
  getActiveConfig(): SyncConfig | undefined;
  setActiveConfig(config: SyncConfig | undefined): void;
  getActivePreferences(): AccountPreferences;
  getUsername(): string;
  setUsername(name: string): void;
  /** 保存会话配置；退出账号传空 token：Rust 侧视为未登录（没有活动档案）。 */
  persistSyncConfig(config: SyncConfig | null): Promise<void>;
  /** 保存偏好到当前活动档案；偏好跟账号走，与全局会话配置分开持久化。 */
  persistAccountPreferences(preferences: AccountPreferences): Promise<void>;
  /** 偏好热更新：自动上传档位是 SyncClient 构造时固化的，经此运行期下发。 */
  applyAutoUploadLimit(limitMb: number): void;
  /** 断开当前同步客户端。 */
  disconnect(): void;
  /** 退出账号后清除「已保存过配置」标记，登录页不再提供返回主界面的入口。 */
  markSignedOut(): void;
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
const autoUploadLimitMb = ref(DEFAULT_AUTO_UPLOAD_LIMIT_MB);
const autoReceiveClipboard = ref(DEFAULT_AUTO_RECEIVE_CLIPBOARD);
/** 文本域里的过滤模式，一行一条；保存时拆成数组。 */
const excludePatternsInput = ref("");
/** 服务器单文件存储上限（MB），登录时下发；自动上传档位不能超过它。 */
const serverMaxFileMb = ref(DEFAULT_SERVER_MAX_FILE_MB);
/** 设备别名草稿；空串表示未设置，展示回退到系统机器名。 */
const deviceAliasInput = ref("");
const savedDeviceAlias = ref("");
/** 系统机器名，作别名输入框的 placeholder。 */
const systemDeviceName = ref("");
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
  const preferences = requireBridge().getActivePreferences();
  autoUploadLimitMb.value = preferences.autoUploadLimitMb;
  autoReceiveClipboard.value = preferences.autoReceiveClipboard;
  excludePatternsInput.value = preferences.excludePatterns.join("\n");
  serverMaxFileMb.value = preferences.serverMaxFileMb;
  // 服务器上限被调低后，已保存的档位可能超出：打开设置时先压回去。
  if (autoUploadLimitMb.value > serverMaxFileMb.value) {
    autoUploadLimitMb.value = serverMaxFileMb.value;
  }
  void loadDeviceIdentity();
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

async function loadDeviceIdentity(): Promise<void> {
  try {
    const identity = await getDeviceIdentity();
    savedDeviceAlias.value = identity.deviceAlias;
    deviceAliasInput.value = identity.deviceAlias;
    systemDeviceName.value = identity.systemDeviceName;
  } catch {
    systemDeviceName.value = "";
  }
}

async function openAppDataDirectory(): Promise<void> {
  settingsError.value = "";
  try {
    await invoke("open_app_data_dir");
  } catch (error) {
    settingsError.value = `无法打开应用数据目录：${errorMessage(error)}`;
  }
}

async function saveSettings(): Promise<void> {
  const activeConfig = requireBridge().getActiveConfig();
  if (!activeConfig || savingSettings.value || changingPassword.value) return;
  const { platformCapabilities } = usePlatform();
  savingSettings.value = true;
  settingsError.value = "";
  const activePreferences = requireBridge().getActivePreferences();
  const preferences: AccountPreferences = {
    autoUploadLimitMb: autoUploadLimitMb.value,
    autoReceiveClipboard: autoReceiveClipboard.value,
    excludePatterns: excludePatternsInput.value
      .split("\n")
      .map((pattern) => pattern.trim())
      .filter((pattern, index, all) => pattern !== "" && all.indexOf(pattern) === index),
    serverMaxFileMb: activePreferences.serverMaxFileMb,
    maxCaptureFileCount: activePreferences.maxCaptureFileCount,
  };
  try {
    // 别名变化先落 device.json，再走 HTTP 立即上报服务器；旧服务器没有该
    // 端点时忽略，随后的 startSync 重连仍会带着新名字兜底。
    const deviceAlias = deviceAliasInput.value.trim();
    if (deviceAlias !== savedDeviceAlias.value) {
      await invoke("save_device_alias", { alias: deviceAlias });
      savedDeviceAlias.value = deviceAlias;
      try {
        await pushDeviceInfo(activeConfig.serverAddress, activeConfig.serverProtocol, activeConfig.sessionToken, await getDevice());
      } catch {
        // 上报失败不阻塞保存：重连时的 WS auth 消息仍会携带设备信息。
      }
    }
    if (
      platformCapabilities.value.globalShortcut
      && settingsPage.value === "shortcuts"
      && !(await saveQuickPasteShortcut())
    ) {
      settingsError.value = quickPasteShortcutStatus.value.message;
      return;
    }
    // 偏好落当前活动档案。Rust 侧 save_account_preferences 即时更新内存，
    // 捕获路径（过滤规则、复制上限）无需重连即生效；自动上传档位经 setter
    // 下发给运行中的 SyncClient。均不需要断开重连 WS。
    await requireBridge().persistAccountPreferences(preferences);
    requireBridge().applyAutoUploadLimit(preferences.autoUploadLimitMb);
    settingsVisible.value = false;
    await nextTick();
    await requireBridge().focusSearch();
  } catch (error) {
    settingsError.value = `无法保存设置：${errorMessage(error)}`;
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
      sessionToken: "",
    };
    await requireBridge().persistSyncConfig(config);
    requireBridge().setActiveConfig(config);
    requireBridge().setUsername(config.username);
    requireBridge().disconnect();
    settingsVisible.value = false;
    clearPasswordChangeFields();
    requireBridge().openSetup({
      config,
      message: "密码已修改，请使用新密码重新登录",
      focus: "password",
    });
  } catch (error) {
    settingsError.value = `修改密码失败：${errorMessage(error)}`;
  } finally {
    changingPassword.value = false;
  }
}

async function signOut(): Promise<void> {
  const activeConfig = requireBridge().getActiveConfig();
  if (!activeConfig || savingSettings.value || changingPassword.value) return;
  savingSettings.value = true;
  settingsError.value = "";
  // 只清 sessionToken：服务器地址与用户名保留在配置里，登录页据此回填。
  // Rust 侧把空 token 视为未登录——没有活动档案，捕获与查询一并停用。
  const signedOutConfig: SyncConfig = { ...activeConfig, sessionToken: "" };
  try {
    await requireBridge().persistSyncConfig(signedOutConfig);
    requireBridge().setActiveConfig(undefined);
    requireBridge().setUsername("");
    requireBridge().disconnect();
    // 退出即回到登录页；清掉「已保存配置」标记，登录页因此不提供
    // 返回主界面的入口。
    requireBridge().markSignedOut();
    settingsVisible.value = false;
    requireBridge().openSetup({ config: signedOutConfig, focus: "server" });
  } catch (error) {
    settingsError.value = `无法退出账号：${errorMessage(error)}`;
  } finally {
    savingSettings.value = false;
  }
}

export {
  settingsVisible,
  settingsPage,
  autoUploadLimitMb,
  autoReceiveClipboard,
  excludePatternsInput,
  serverMaxFileMb,
  deviceAliasInput,
  systemDeviceName,
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
