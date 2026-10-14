import { errorMessage } from "../../utils";
import { getDevice, getDeviceIdentity, saveDeviceAlias } from "../../platform/storage";
import { nextTick } from "vue";
import { accountStateRef, getAccountSession } from "../../accountSession";
import { changeAccountPassword, pushDeviceInfo } from "../sync/authApi";
import type { AccountPreferences, SettingsPage, SyncConfig } from "../../types";

/**
 * 设置弹窗的模块级单例（对齐 usePlatform 风格）。弹窗状态被工作区菜单和
 * 全局键盘（Esc）共享，所以提升到
 * 模块层；触达同步引擎的部分通过 App.vue 注入的 bridge 完成。
 */
export type SettingsBridge = {
  getActiveConfig(): SyncConfig | undefined;
  setActiveConfig(config: SyncConfig | undefined): void;
  getActivePreferences(): AccountPreferences;
  setUsername(name: string): void;
  /** 保存会话配置；退出账号传空 token：Rust 侧视为未登录（没有活动档案）。 */
  persistSyncConfig(config: SyncConfig | null): Promise<void>;
  /** 保存偏好到当前活动档案；偏好跟账号走，与全局会话配置分开持久化。 */
  persistAccountPreferences(preferences: AccountPreferences): Promise<void>;
  /** 偏好热更新：自动上传档位是 账号连接创建时固化的，经此运行期下发。 */
  applyAutoUploadLimit(limitMb: number): void;
  /** 断开当前同步客户端。 */
  disconnect(): Promise<void>;
  /** 退出账号后清除「已保存过配置」标记，登录页不再提供返回主界面的入口。 */
  markSignedOut(): void;
  openSetup(o: { config?: SyncConfig; message?: string; focus?: "server" | "password" }): void;
  focusSearchInput(): void;
};

let bridge: SettingsBridge | undefined;

export function initSettings(settingsBridge: SettingsBridge): void {
  bridge = settingsBridge;
}

function requireBridge(): SettingsBridge {
  if (!bridge) throw new Error("Settings bridge 尚未初始化（应先在 App.vue 中调用 initSettings）");
  return bridge;
}

const settingsVisible = accountStateRef("settingsVisible");
const settingsPage = accountStateRef("settingsPage");
const deviceAliasInput = accountStateRef("deviceAliasInput");
const systemDeviceName = accountStateRef("systemDeviceName");
const savingSettings = accountStateRef("savingSettings");
const changingPassword = accountStateRef("changingPassword");
const settingsError = accountStateRef("settingsError");
const passwordChangeError = accountStateRef("passwordChangeError");
const currentPassword = accountStateRef("currentPassword");
const newPassword = accountStateRef("newPassword");
const confirmNewPassword = accountStateRef("confirmNewPassword");

function cancelSettingsRequests(): void {
  getAccountSession()?.state.settingsRequestController?.abort();
}

function openSettings(): void {
  const activeConfig = requireBridge().getActiveConfig();
  if (!activeConfig) return;
  void loadDeviceIdentity();
  settingsPage.value = "general";
  clearPasswordChangeFields();
  settingsError.value = "";
  settingsVisible.value = true;
}

function selectSettingsPage(page: SettingsPage): void {
  settingsPage.value = page;
  settingsError.value = "";
}

function closeSettings(): void {
  if (savingSettings.value || changingPassword.value) return;
  settingsVisible.value = false;
  settingsError.value = "";
  clearPasswordChangeFields();
  void nextTick(() => { void requireBridge().focusSearchInput(); });
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
  const session = getAccountSession();
  if (!session) return;
  const { deviceAliasInput, savedDeviceAlias, systemDeviceName } = session.state;

  try {
    const identity = await session.start(() => getDeviceIdentity());
    savedDeviceAlias.value = identity.deviceAlias;
    deviceAliasInput.value = identity.deviceAlias;
    systemDeviceName.value = identity.systemDeviceName;
  } catch {
    systemDeviceName.value = "";
  }
}

export function getSettingsPreferences(): AccountPreferences {
  return requireBridge().getActivePreferences();
}

async function saveSettings(options: {preferences?: AccountPreferences; beforeSave?: () => Promise<void>} = {}): Promise<void> {
  const session = getAccountSession();
  if (!session) return;
  const { settingsVisible, deviceAliasInput, savedDeviceAlias, savingSettings, changingPassword, settingsError } = session.state;

  const activeConfig = requireBridge().getActiveConfig();
  if (!activeConfig || savingSettings.value || changingPassword.value) return;
  savingSettings.value = true;
  const controller = new AbortController();
  session.state.settingsRequestController = controller;
  session.own(() => controller.abort());
  settingsError.value = "";
  try {
    // 别名变化先落 device.json，再走 HTTP 立即上报服务器。
    const deviceAlias = deviceAliasInput.value.trim();
    if (deviceAlias !== savedDeviceAlias.value) {
      await saveDeviceAlias(deviceAlias);
      savedDeviceAlias.value = deviceAlias;
      try {
        await pushDeviceInfo(activeConfig.serverAddress, activeConfig.serverProtocol, activeConfig.sessionToken, await session.start(() => getDevice()), controller.signal);
      } catch {
        // 上报失败不阻塞保存：下次启动同步时的会话验证会重新读取并上报设备信息。
      }
    }
    await options.beforeSave?.();
    const preferences = options.preferences;
    if (preferences) {
      await session.start(() => requireBridge().persistAccountPreferences(preferences));
      requireBridge().applyAutoUploadLimit(preferences.autoUploadLimitMb);
    }
    settingsVisible.value = false;
    await nextTick();
    await requireBridge().focusSearchInput();
  } catch (error) {
    settingsError.value = `无法保存设置：${errorMessage(error)}`;
  } finally {
    savingSettings.value = false;
    if (session.state.settingsRequestController === controller) session.state.settingsRequestController = undefined;
  }
}

async function changePassword(): Promise<void> {
  const session = getAccountSession();
  if (!session) return;
  const { settingsVisible, savingSettings, changingPassword, settingsError, passwordChangeError, currentPassword, newPassword } = session.state;

  const activeConfig = requireBridge().getActiveConfig();
  if (!activeConfig?.username || changingPassword.value || savingSettings.value) return;
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
  const controller = new AbortController();
  session.state.settingsRequestController = controller;
  session.own(() => controller.abort());
  settingsError.value = "";
  try {
    await changeAccountPassword(
      activeConfig.serverAddress,
      activeConfig.serverProtocol,
      activeConfig.sessionToken,
      currentPassword.value,
      newPassword.value,
      controller.signal,
    );
    const config: SyncConfig = {
      ...activeConfig,
      sessionToken: "",
    };
    await session.start(() => requireBridge().persistSyncConfig(config));
    requireBridge().setActiveConfig(config);
    requireBridge().setUsername(config.username);
    await requireBridge().disconnect();
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
    if (session.state.settingsRequestController === controller) session.state.settingsRequestController = undefined;
  }
}

async function signOut(switchingServer = false): Promise<void> {
  const session = getAccountSession();
  if (!session) return;
  const { settingsVisible, savingSettings, changingPassword, settingsError } = session.state;

  const activeConfig = requireBridge().getActiveConfig();
  if (!activeConfig || savingSettings.value || changingPassword.value) return;
  savingSettings.value = true;
  settingsError.value = "";
  // 只清 sessionToken：服务器地址与用户名保留在配置里，登录页据此回填。
  // Rust 侧把空 token 视为未登录——没有活动档案，捕获与查询一并停用。
  const signedOutConfig: SyncConfig = { ...activeConfig, sessionToken: "" };
  try {
    await session.start(() => requireBridge().persistSyncConfig(signedOutConfig));
    requireBridge().setActiveConfig(undefined);
    requireBridge().setUsername("");
    await requireBridge().disconnect();
    // 退出即回到登录页；清掉「已保存配置」标记，登录页因此不提供
    // 返回主界面的入口。
    requireBridge().markSignedOut();
    settingsVisible.value = false;
    requireBridge().openSetup({
      config: switchingServer ? { ...signedOutConfig, serverAddress: "", username: "" } : signedOutConfig,
      message: switchingServer ? "请输入新的服务器地址并登录" : "",
      focus: "server",
    });
  } catch (error) {
    settingsError.value = `无法退出账号：${errorMessage(error)}`;
  } finally {
    savingSettings.value = false;
  }
}

function switchServer(): Promise<void> {
  return signOut(true);
}

export {
  settingsVisible,
  settingsPage,
  deviceAliasInput,
  systemDeviceName,
  savingSettings,
  changingPassword,
  settingsError,
  passwordChangeError,
  currentPassword,
  newPassword,
  confirmNewPassword,
  openSettings,
  selectSettingsPage,
  closeSettings,
  validateNewPassword,
  validatePasswordConfirmation,
  saveSettings,
  changePassword,
  signOut,
  switchServer,
  cancelSettingsRequests,
};
