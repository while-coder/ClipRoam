import { ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { activeAccountSession } from "@cliproam/ui/account-session";
import { errorMessage } from "@cliproam/ui/errors";

export const iosShortcutsAvailable = ref(false);
export const iosShortcutsReady = ref(false);
export const iosShortcutsError = ref("");
export const iosQuickAddSupported = ref(false);
export const iosQuickAdding = ref(false);
export const iosQuickAddError = ref("");

// Public template only: account credentials remain in Keychain, never in this URL.
const smsShortcutUrl = import.meta.env.VITE_IOS_SMS_SHORTCUT_URL?.trim() ?? "";
export const iosQuickAddConfigured = isShortcutShareUrl(smsShortcutUrl);

function isShortcutShareUrl(value: string): boolean {
  return /^https:\/\/www\.icloud\.com\/shortcuts\/[a-fA-F0-9]{32}\/?$/.test(value);
}

export async function quickAddIosSmsShortcut(): Promise<void> {
  if (iosQuickAdding.value) return;
  iosQuickAddError.value = "";
  if (!iosQuickAddSupported.value || !iosQuickAddConfigured || !iosShortcutsReady.value) return;
  iosQuickAdding.value = true;
  try { await invoke("native_add_ios_sms_shortcut", { url: smsShortcutUrl }); }
  catch (error) { iosQuickAddError.value = `无法打开快捷指令添加页面：${errorMessage(error)}`; }
  finally { iosQuickAdding.value = false; }
}
const reconfiguration = ref(0);
export function reconfigureIosShortcuts(): void { reconfiguration.value++; }

export function initializeIosShortcuts(osVersion: string): () => void {
  iosShortcutsAvailable.value = true;
  iosQuickAddSupported.value = Number.parseInt(osVersion, 10) >= 27;
  let disposed = false;
  let queue = Promise.resolve();
  const stop = watch([activeAccountSession, reconfiguration], ([owner, revision], [previous, previousRevision]) => {
    // A cold launch has not restored its session yet; keep the native credentials
    // available to background intents until an actual login/logout transition.
    if (!owner && !previous) return;
    iosShortcutsReady.value = false;
    iosShortcutsError.value = "";
    // Serialize account changes so an older write cannot restore a logged-out token.
    queue = queue.then(async () => {
      if (disposed || owner !== activeAccountSession.value) return;
      try {
        if (previousRevision !== undefined && revision !== previousRevision) {
          await invoke("native_configure_ios_shortcuts", { serverAddress: null, sessionToken: null });
        }
        await invoke("native_configure_ios_shortcuts", {
          serverAddress: owner?.config.serverAddress ?? null,
          sessionToken: owner?.config.sessionToken ?? null,
        });
        if (owner === activeAccountSession.value) iosShortcutsReady.value = Boolean(owner);
      } catch (error) {
        if (owner === activeAccountSession.value) iosShortcutsError.value = errorMessage(error);
      }
    });
  }, { immediate: true, flush: "sync" });
  return () => { disposed = true; stop(); };
}
