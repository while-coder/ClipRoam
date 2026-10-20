import { onBeforeUnmount, onMounted } from "vue";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getAccountSession } from "@cliproam/ui/account-session";
import { errorMessage } from "@cliproam/ui/errors";
import { native, type Unlisten } from "@cliproam/ui/native";
import { isNative, isQuickPaste, showNotification } from "@cliproam/ui/platform";

/** Native drops contain paths, so folders use the same capture flow as the picker. */
export function useFileDropUpload(): void {
  let disposed = false;
  let unlisten: Unlisten | undefined;

  async function capturePaths(paths: string[]): Promise<void> {
    if (disposed || isQuickPaste.value || !paths.length) return;
    const session = getAccountSession();
    if (!session || session.signal.aborted) {
      showNotification({ message: "请先登录账号后再拖入文件", tone: "info" });
      return;
    }
    try {
      await session.ready;
      session.signal.throwIfAborted();
      await session.capture.capture({ kind: "files", paths });
      if (!disposed && !session.signal.aborted) {
        showNotification({ message: "已添加到待上传列表", tone: "success" });
      }
    } catch (error) {
      if (!disposed && !session.signal.aborted) {
        showNotification({ message: `拖入文件失败：${errorMessage(error)}`, tone: "error" });
      }
    }
  }

  onMounted(async () => {
    if (!isNative || native.windowLabel !== "main") return;
    try {
      if ((await native.getCapabilities()).mobile || disposed) return;
      const cleanup = await getCurrentWebview().onDragDropEvent(({ payload }) => {
        if (payload.type === "drop") void capturePaths(payload.paths);
      });
      if (disposed) cleanup();
      else unlisten = cleanup;
    } catch (error) {
      console.warn("Could not initialize file drop uploads", error);
    }
  });

  onBeforeUnmount(() => {
    disposed = true;
    unlisten?.();
  });
}
