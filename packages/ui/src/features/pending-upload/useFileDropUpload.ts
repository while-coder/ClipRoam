import { onBeforeUnmount, onMounted } from "vue";
import { getAccountSession } from "../../accountSession";
import { isNative, isQuickPaste, showNotification } from "../../platform/runtime";
import { native, type PickedFiles, type Unlisten } from "../../platform/native";
import { errorMessage } from "../../utils";

/** Native paths and browser Files share account checks, capture and queue feedback. */
async function captureDroppedFiles(picked: PickedFiles | ((signal: AbortSignal) => Promise<PickedFiles>)): Promise<void> {
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

/** Read every directory batch; Chromium returns at most 100 entries per batch. */
async function collectEntry(entry: FileSystemEntry, path: string, files: File[], signal: AbortSignal): Promise<void> {
  signal.throwIfAborted();
  if (entry.isFile) {
    const file = await new Promise<File>((resolve, reject) => (entry as FileSystemFileEntry).file(resolve, reject));
    Object.defineProperty(file, "webkitRelativePath", { value: path });
    files.push(file);
  } else if (entry.isDirectory) {
    const reader = (entry as FileSystemDirectoryEntry).createReader();
    for (;;) {
      signal.throwIfAborted();
      const children = await new Promise<FileSystemEntry[]>((resolve, reject) => reader.readEntries(resolve, reject));
      if (!children.length) break;
      for (const child of children) await collectEntry(child, `${path}/${child.name}`, files, signal);
    }
  }
}

/** One upload hook for native paths and browser Files (PWA/VS Code/extensions). */
export function useFileDropUpload(): void {
  let disposed = false;
  let unlisten: Unlisten | undefined;
  const hasFiles = (event: DragEvent): boolean => Boolean(event.dataTransfer?.types.includes("Files"));
  const dragOver = (event: DragEvent): void => {
    if (!hasFiles(event)) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "copy";
  };
  const drop = (event: DragEvent): void => {
    if (!hasFiles(event) || !event.dataTransfer) return;
    event.preventDefault();
    // Snapshot entries and Files synchronously, while the drag data store is readable.
    const items = Array.from(event.dataTransfer.items).filter(item => item.kind === "file")
      .map(item => ({ entry: item.webkitGetAsEntry?.(), file: item.getAsFile() }));
    const fallback = Array.from(event.dataTransfer.files);
    void captureDroppedFiles(async signal => {
      if (!items.length) return { files: fallback };
      const files: File[] = [];
      for (const item of items) {
        if (item.entry) await collectEntry(item.entry, item.entry.name, files, signal);
        else if (item.file) files.push(item.file);
      }
      return { files };
    });
  };
  onMounted(async () => {
    if (isNative) {
      try {
        const cleanup = await native.onFileDrop?.((picked) => {
          if (!disposed) void captureDroppedFiles(picked);
        });
        if (disposed) cleanup?.();
        else unlisten = cleanup;
      } catch (error) {
        console.warn("Could not initialize file drop uploads", error);
      }
      return;
    }
    window.addEventListener("dragover", dragOver);
    window.addEventListener("drop", drop);
  });
  onBeforeUnmount(() => {
    disposed = true;
    unlisten?.();
    window.removeEventListener("dragover", dragOver);
    window.removeEventListener("drop", drop);
  });
}
