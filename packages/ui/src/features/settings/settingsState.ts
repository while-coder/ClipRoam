import { ref } from "vue";
import type { SettingsPage } from "../../types";

export function createSettingsState() {
  const settingsVisible = ref(false);
  const settingsPage = ref<SettingsPage>("general");
  /** 设备别名草稿；空串表示未设置，展示回退到系统机器名。 */
  const deviceAliasInput = ref("");
  const savedDeviceAlias = ref("");
  /** 系统机器名，作别名输入框的 placeholder。 */
  const systemDeviceName = ref("");
  const savingSettings = ref(false);
  const changingPassword = ref(false);
  const settingsError = ref("");
  const passwordChangeError = ref("");
  const currentPassword = ref("");
  const newPassword = ref("");
  const confirmNewPassword = ref("");
  return { settingsVisible, settingsPage, deviceAliasInput, savedDeviceAlias, systemDeviceName, savingSettings, changingPassword, settingsError, passwordChangeError, currentPassword, newPassword, confirmNewPassword };
}
