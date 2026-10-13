import { ref } from "vue";
import { native } from "../../platform/native";
import packageInfo from "../../../package.json";

const fallbackAppVersion = packageInfo.version;
const appVersion = ref("");
let versionInitialized = false;

async function initAppVersion(): Promise<void> {
  if (versionInitialized) return;
  versionInitialized = true;

  try {
    appVersion.value = (await native.getAppVersion()) || fallbackAppVersion;
  } catch {
    appVersion.value = fallbackAppVersion;
  }
}

export function useAppVersion() {
  return {
    appVersion,
    initAppVersion,
  };
}
