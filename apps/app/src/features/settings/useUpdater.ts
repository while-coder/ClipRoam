import { ref } from "vue";
import { getVersion } from "@tauri-apps/api/app";
import { useTauriUpdater } from "@while-coder/tauri-updater-vue";
import packageInfo from "../../../package.json";

const updater = useTauriUpdater();
const fallbackAppVersion = packageInfo.version;
const appVersion = ref(updater.runningInTauri ? "" : fallbackAppVersion);
let versionInitialized = false;

async function initUpdaterVersion(): Promise<void> {
  if (versionInitialized) return;
  versionInitialized = true;

  // Outside Tauri the ref already holds the fallback version.
  if (!updater.runningInTauri) return;

  try {
    appVersion.value = (await getVersion()) || fallbackAppVersion;
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
