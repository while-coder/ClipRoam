import { watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { activeAccountSession } from "@cliproam/ui/account-session";
import { showNotification } from "@cliproam/ui/platform";
import { errorMessage } from "@cliproam/ui/errors";

/** Share the active account with WidgetKit; the extension pulls its own snapshots. */
export function initializeIosWidget(): () => void {
  let disposed = false;
  let queue = Promise.resolve();
  const configure = () => {
    const owner = activeAccountSession.value;
    queue = queue.then(async () => {
      if (disposed || owner !== activeAccountSession.value) return;
      try {
        await invoke("native_configure_ios_widget", {
          serverAddress: owner?.config.serverAddress ?? null,
          sessionToken: owner?.config.sessionToken ?? null,
        });
      } catch (error) {
        if (owner === activeAccountSession.value) {
          showNotification({ message: `便签小组件配置失败：${errorMessage(error)}`, tone: "error" });
        }
      }
    });
  };
  const stop = watch(activeAccountSession, (owner, previous, onCleanup) => {
    // Preserve the last account while the app restores its session on cold launch.
    if (!owner && !previous) return;
    configure();
    if (!owner) return;
    const changed = () => owner.schedule("ios-widget-refresh", () => {
      if (!disposed && owner === activeAccountSession.value) configure();
    }, 1000);
    owner.history.addEvent(changed);
    onCleanup(() => owner.history.delEvent(changed));
  }, { immediate: true, flush: "sync" });
  return () => { disposed = true; stop(); };
}
