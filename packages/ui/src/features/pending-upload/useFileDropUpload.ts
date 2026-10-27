import { onBeforeUnmount, onMounted } from "vue";
import { getAccountSession } from "../../accountSession";
import { isQuickPaste, showNotification } from "../../platform/runtime";
import { native, type DroppedFiles, type Unlisten } from "../../platform/native";
import { errorMessage } from "../../utils";

/** Native paths and browser Files share account checks, capture and queue feedback. */
async function captureDroppedFiles(picked: DroppedFiles): Promise<void> {
  if (isQuickPaste.value) return;
  const session = getAccountSession();
  if (!session || session.signal.aborted) {
    showNotification({ message: "请先登录账号后再拖入文件", tone: "info" });
    return;
  }
  try {
    await session.ready;
    session.signal.throwIfAborted();
    const files = typeof picked === "function" ? await picked(session.signal) : picked;
    session.signal.throwIfAborted();
    if (files.files?.length) await session.capture.captureFiles(files.files);
    else if (files.paths?.length) await session.capture.capture({ kind: "files", paths: files.paths });
    else throw new Error("没有可用的文件");
    if (!session.signal.aborted) showNotification({ message: "已添加到待上传列表", tone: "success" });
  } catch (error) {
    if (!session.signal.aborted) showNotification({ message: `拖入文件失败：${errorMessage(error)}`, tone: "error" });
  }
}

/** One upload hook for native paths and browser Files (PWA/VS Code/extensions). */
export function useFileDropUpload(): void {
  let disposed = false;
  let unlisten: Unlisten | undefined;
  onMounted(async () => {
    try {
      const cleanup = await native.onFileDrop((picked) => {
        if (!disposed) void captureDroppedFiles(picked);
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
