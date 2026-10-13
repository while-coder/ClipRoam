import { ref } from "vue";
import { native, type UpdaterState } from "../../platform/native";
import packageInfo from "../../../package.json";

const updater: UpdaterState = native.updater ?? {
  updaterSupported: false,
  updateStatus: ref("idle"),
  updateStatusText: ref("当前平台不支持应用内更新"),
  checkForUpdate: async () => "unsupported",
};
const fallbackAppVersion = packageInfo.version;
const appVersion = ref("");
let versionInitialized = false;

async function initUpdaterVersion(): Promise<void> {
  if (versionInitialized) return;
  versionInitialized = true;

  try {
    appVersion.value = (await native.getAppVersion()) || fallbackAppVersion;
  } catch {
    appVersion.value = fallbackAppVersion;
  }
}

export function useUpdater() {
  return {
    ...updater,
    appVersion,
    initUpdaterVersion,
  };
}
