import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { ClipboardEntry } from "@cliproam/protocol";
import { isMobile, isPasteWindow, runningInTauri } from "../../composables/usePlatform";
import { showToast } from "../toast/useToast";
import { errorMessage } from "../../utils/error";
import { canSaveEntry } from "../../utils/entry";
import { DownloadCancelledError } from "../sync/fileTransfer";
import { getSyncClient } from "../sync/syncEngine";
import { downloader, ensureLocalFiles, ensurePasteReady } from "../downloads/useDownloads";
import { refreshHistory } from "./useHistorySync";
import type { LocalClipboardEntry, SavePreparation } from "../../types";

/**
 * 条目动作（从 App.vue 下沉）：复制/粘贴/另存/删除。数据落地经 Downloader
 * 与 Rust 命令，历史失效走 refreshHistory。
 */
export const activatingEntryIds = ref(new Set<string>());
export const savingEntryId = ref("");
/** 剪贴板写入串行化：下载可并发，落剪贴板同一时刻只允许一个 invoke。 */
let clipboardWriteChain: Promise<unknown> = Promise.resolve();

async function activateEntry(
  entry: LocalClipboardEntry | undefined,
  command: "copy_entry" | "paste_entry",
): Promise<void> {
  if (!entry) return;
  if (!runningInTauri) {
    await navigator.clipboard.writeText(entry.content);
    showToast("已复制到系统剪贴板", "success");
    return;
  }
  if (isMobile.value && entry.kind !== "text") {
    await saveEntry(entry);
    return;
  }
  if (activatingEntryIds.value.has(entry.id)) {
    // 下载中再次激活 = 取消该下载；无下载（如纯文本快速粘贴）则维持防重复。
    if (await downloader.cancelEntry(entry.id) > 0) showToast("已取消下载", "info");
    return;
  }
  activatingEntryIds.value.add(entry.id);
  try {
    // Rust selects the native strategy. This downloads only what the current
    // platform must materialize before it can copy or paste the entry.
    await ensurePasteReady(entry);
    // 串行化：等待轮到自己再写剪贴板，避免并发 paste_entry 交错。
    const write = clipboardWriteChain.then(() => invoke(command, { entryId: entry.id }));
    clipboardWriteChain = write.catch(() => undefined);
    await write;
    // 粘贴的结果用户肉眼可见（内容已进入目标应用），不再弹提示；复制的结果
    // 看不见，保留确认提示。
    if (command === "copy_entry") showToast("已复制到系统剪贴板", "success");
  } catch (error) {
    // 取消的提示已由取消方给出，原激活方静默收尾。
    if (error instanceof DownloadCancelledError) return;
    if (String(error).includes("clipboard entry was not found")) {
      refreshHistory();
      return;
    }
    showToast(String(error), "error");
  } finally {
    activatingEntryIds.value.delete(entry.id);
  }
}

function copyEntry(entry?: LocalClipboardEntry): Promise<void> {
  return activateEntry(entry, "copy_entry");
}

function pasteEntry(entry?: LocalClipboardEntry): Promise<void> {
  return activateEntry(entry, "paste_entry");
}

export async function saveEntry(entry: LocalClipboardEntry): Promise<void> {
  if (savingEntryId.value || !canSaveEntry(entry)) return;
  savingEntryId.value = entry.id;
  let saveId: string | undefined;
  try {
    if (isMobile.value) {
      await ensureLocalFiles(entry);
      showToast("内容已下载到应用缓存，可在 ClipRoam 中离线使用", "success");
    } else {
      const preparation = await invoke<SavePreparation | null>("prepare_save_entry", {
        entryId: entry.id,
      });
      if (!preparation) return;
      saveId = preparation.saveId;

      if (preparation.missing.length) {
        // saveId 隔离任务：落盘到另存 staging 而非内容寻址缓存。
        await downloader.downloadFiles(entry.id, preparation.missing, {
          saveId: preparation.saveId,
          entryLabel: entry.content,
        });
      }

      const saved = await invoke<number>("finish_save_entry", { saveId: preparation.saveId });
      saveId = undefined;
      if (saved > 0) showToast(`已保存 ${saved} 个文件`, "success");
    }
  } catch (error) {
    if (error instanceof DownloadCancelledError) return;
    if (saveId) await invoke("cancel_save_entry", { saveId }).catch(() => undefined);
    showToast(`${isMobile.value ? "下载" : "另存为"}失败：${errorMessage(error)}`, "error");
  } finally {
    savingEntryId.value = "";
  }
}

/**
 * Activation requests from the history view. `viaClick` mirrors the old
 * select-or-activate split: clicks activate immediately only in the paste
 * window and on mobile, keyboard/double-click everywhere.
 */
export function activateFromView(entry: LocalClipboardEntry, viaClick: boolean): void {
  if (viaClick) {
    if (isPasteWindow) void pasteEntry(entry);
    else if (isMobile.value) void copyEntry(entry);
    return;
  }
  if (isPasteWindow) void pasteEntry(entry);
  else if (entry.kind === "files") {
    showToast("文件请使用 Ctrl+Shift+V 快捷粘贴，或点击“另存为…”手动下载", "info");
  } else {
    void copyEntry(entry);
  }
}

/** A pending row's display id is `p{seq}`; extract the seq its Dequeue takes. */
function pendingSeqOf(entryId: string): number | null {
  return /^p\d+$/.test(entryId) ? Number(entryId.slice(1)) : null;
}

// Deletion is server-authoritative: the request goes out, and the local entry
// is only cleaned up when the `clipboard.deleted` echo arrives (the server
// broadcasts to every device, including the initiator). A pending queue row
// never reached the server, so it is dequeued outright.
export async function removeEntry(entry: ClipboardEntry): Promise<void> {
  const seq = pendingSeqOf(entry.id);
  if (seq !== null) {
    await invoke("dequeue_pending_entry", { seq }).catch((error) => {
      showToast(`删除失败：${errorMessage(error)}`, "error");
    });
    return;
  }
  const client = getSyncClient();
  if (!client) {
    showToast("网络异常，暂时无法删除，请检查同步连接", "error");
    return;
  }
  try {
    await client.delete(entry.id);
  } catch (error) {
    showToast(`删除失败：${errorMessage(error)}`, "error");
    return;
  }
  // Idempotent fallback in case the echo is lost (e.g. disconnect right after
  // the response); cleanup stays a no-op if the echo already handled it.
  if (runningInTauri) {
    setTimeout(() => {
      // The command emits `cliproam://history-changed`, which refreshes the
      // views; no explicit invalidation needed here.
      void invoke("remove_server_entry", { entryId: entry.id });
    }, 5000);
  }
}
