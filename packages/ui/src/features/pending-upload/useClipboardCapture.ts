import { ref } from "vue";
import { getAccountSession } from "../sync/accountSession";
import { showToast } from "../../core/toast";
import { errorMessage } from "../../utils/error";

// Shared across workspace pages so switching pages cannot open another picker.
export const capturingClipboard = ref(false);
export const uploadingFiles = ref(false);

export async function captureCurrentClipboard(): Promise<void> {
  if (capturingClipboard.value) return;
  capturingClipboard.value = true;
  try {
    const captured = await getAccountSession()?.capture.readCurrentText();
    showToast(captured ? "已读取当前文本剪贴板" : "当前剪贴板没有可读取的文本", captured ? "success" : "info");
  } catch (error) {
    showToast(`读取剪贴板失败：${errorMessage(error)}`, "error");
  } finally {
    capturingClipboard.value = false;
  }
}

export async function captureFilesFromPicker(mode: "file" | "folder"): Promise<void> {
  if (uploadingFiles.value) return;
  uploadingFiles.value = true;
  try {
    const captured = await getAccountSession()?.capture.pickFiles(mode);
    showToast(captured ? "已加入上传队列" : "未选择文件", captured ? "success" : "info");
  } catch (error) {
    showToast(`选择文件失败：${errorMessage(error)}`, "error");
  } finally {
    uploadingFiles.value = false;
  }
}
