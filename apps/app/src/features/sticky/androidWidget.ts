import { watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { activeAccountSession } from "@cliproam/ui/account-session";
import { showNotification } from "@cliproam/ui/platform";
import { errorMessage } from "@cliproam/ui/errors";

/** Native workers own widget snapshots and continue syncing without the WebView. */
export function initializeAndroidWidget(): () => void {
  let disposed = false;
  let queue = Promise.resolve();
  const configure = () => {
    const owner = activeAccountSession.value;
    queue = queue.then(async () => {
      if (disposed || owner !== activeAccountSession.value) return;
      try {
        await invoke("native_configure_android_widget", {
          serverAddress: owner?.config.serverAddress ?? null,
          sessionToken: owner?.config.sessionToken ?? null,
        });
      } catch (error) {
        if (!disposed && owner === activeAccountSession.value) {
          showNotification({ message: `便签小组件配置失败：${errorMessage(error)}`, tone: "error" });
        }
      }
    });
  };
  const stop = watch(activeAccountSession, (owner, previous, onCleanup) => {
    // A cold launch must not erase the native session before login is restored.
    if (!owner && !previous) return;
    configure();
    if (!owner) return;
    const changed = () => owner.schedule("android-widget-refresh", configure, 1000);
    owner.history.addEvent(changed);
    onCleanup(() => {
      owner.history.delEvent(changed);
      owner.cancelScheduled("android-widget-refresh");
    });
  }, { immediate: true, flush: "sync" });
  return () => { disposed = true; stop(); };
}
