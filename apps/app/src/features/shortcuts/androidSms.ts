import { watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { activeAccountSession } from "@cliproam/ui/account-session";
import { showNotification } from "@cliproam/ui/platform";
import { errorMessage } from "@cliproam/ui/errors";

/** Configure once per login transition; the native receiver owns background uploads. */
export function initializeAndroidSms(): () => void {
  let disposed = false;
  let queue = Promise.resolve();
  const stop = watch(activeAccountSession, (owner, previous) => {
    // Keep persisted credentials while a cold launch is restoring its login.
    if (!owner && !previous) return;
    queue = queue.then(async () => {
      if (disposed || owner !== activeAccountSession.value) return;
      try {
        await invoke("native_configure_android_sms", {
          serverAddress: owner?.config.serverAddress ?? null,
          sessionToken: owner?.config.sessionToken ?? null,
        });
      } catch (error) {
        if (!disposed && owner === activeAccountSession.value) {
          showNotification({ message: `短信同步配置失败：${errorMessage(error)}`, tone: "error" });
        }
      }
    });
  }, { immediate: true, flush: "sync" });
  return () => { disposed = true; stop(); };
}
