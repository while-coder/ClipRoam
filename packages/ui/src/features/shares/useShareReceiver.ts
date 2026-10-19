import { onBeforeUnmount, onMounted, ref, watch, type Ref } from "vue";
import { getAccountSession } from "../../accountSession";
import { shareReceiver } from "../../platform/share";
import { showNotification } from "../../platform/runtime";
import { errorMessage } from "../../utils";
import { importPendingShares } from "./importPendingShares";
import type { Unlisten } from "../../platform/native";
import type { ShareImportSummary } from "../../types";

function importMessage(summary: ShareImportSummary): string {
  const parts = [summary.texts ? `${summary.texts} 条文字` : "", summary.images ? `${summary.images} 张图片` : "", summary.files ? `${summary.files} 个文件` : ""].filter(Boolean);
  return parts.length ? `已接收${parts.join("、")}` : "";
}

/** Import only in the authenticated main view, independent of the source platform. */
export function useShareReceiver(canImport: Readonly<Ref<boolean>>) {
  const importingShare = ref(false);
  let initialized = false;
  let disposed = false;
  let requested = false;
  let unlisten: Unlisten | undefined;
  const reportError = (message: string): void => { if (!disposed) showNotification({ message, tone: "error" }); };

  async function consume(): Promise<void> {
    if (!initialized || disposed || !canImport.value) return;
    requested = true;
    if (importingShare.value) return;
    importingShare.value = true;
    try {
      while (requested && !disposed && canImport.value) {
        requested = false;
        const session = getAccountSession();
        if (!session || session.signal.aborted) break;
        try {
          const summary = await importPendingShares(session, reportError);
          if (!disposed && !session.signal.aborted && summary.shares) showNotification({ message: importMessage(summary), tone: "success" });
        } catch (error) {
          if (!session.signal.aborted) reportError(`接收分享失败：${errorMessage(error)}`);
        }
      }
    } finally { importingShare.value = false; }
  }

  watch(canImport, (ready) => { if (ready) void consume(); }, { flush: "post" });
  onMounted(async () => {
    try {
      if (!await shareReceiver.isSupported() || disposed) return;
      const stop = await shareReceiver.listen((event) => {
        if (event.error) reportError(`接收分享失败：${event.error}，请重新分享`);
        else void consume();
      });
      if (disposed) { stop(); return; }
      unlisten = stop;
      initialized = true;
      await consume();
    } catch (error) { reportError(`分享接收初始化失败：${errorMessage(error)}`); }
  });
  onBeforeUnmount(() => { disposed = true; unlisten?.(); });
  return { importingShare };
}
