import { ref } from "vue";
import type { SettingsPage } from "../../types";
import { DEFAULT_AUTO_RECEIVE_CLIPBOARD, DEFAULT_AUTO_UPLOAD_LIMIT_MB, DEFAULT_SERVER_MAX_FILE_MB, DEFAULT_WATCH_CLIPBOARD } from "../sync/syncDefaults";

export function createSettingsState() {
  const settingsVisible = ref(false);
  const settingsPage = ref<SettingsPage>("general");
  const autoUploadLimitMb = ref(DEFAULT_AUTO_UPLOAD_LIMIT_MB);
  const autoReceiveClipboard = ref(DEFAULT_AUTO_RECEIVE_CLIPBOARD);
  /** 是否监听本机剪贴板；关闭后本机复制不再自动进历史，只收同步内容。 */
  const watchClipboard = ref(DEFAULT_WATCH_CLIPBOARD);
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
  return { settingsVisible, settingsPage, autoUploadLimitMb, autoReceiveClipboard, watchClipboard, excludePatternsInput, serverMaxFileMb, deviceAliasInput, savedDeviceAlias, systemDeviceName, savingSettings, recordingQuickPasteShortcut, changingPassword, settingsError, passwordChangeError, currentPassword, newPassword, confirmNewPassword };
}
