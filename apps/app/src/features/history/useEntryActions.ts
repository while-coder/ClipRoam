import { invoke as nativeInvoke } from "@tauri-apps/api/core";
import { accountStateRef, getAccountSession } from "../sync/accountSession";
import type { ClipboardEntry } from "@cliproam/protocol";
import { isMobile, isPasteWindow, usePlatform } from "../../composables/usePlatform";
import { showToast } from "../toast/useToast";
import { errorMessage } from "../../utils/error";
import { canSaveEntry } from "../../utils/entry";
import { getSyncClient } from "../sync/syncEngine";
import { DownloadCancelledError, cancelEntryDownloads, downloadEntry } from "../downloads/useDownloads";
import { refreshHistory } from "./useHistorySync";
import type { LocalClipboardEntry, SavePreparation } from "../../types";

/**
 * 条目动作（从 App.vue 下沉）：复制/粘贴/另存/删除。文件准备经下载模块与 Rust 命令，历史失效走 refreshHistory。
 */
export const activatingEntryIds = accountStateRef("activatingEntryIds");
export const savingEntryId = accountStateRef("savingEntryId");
const { platformCapabilities } = usePlatform();
const cancelSave = (saveId: string) => nativeInvoke("cancel_save_entry", { saveId }).catch(() => undefined);
async function activateEntry(
  entry: LocalClipboardEntry | undefined,
  command: "copy_entry" | "paste_entry",
): Promise<void> {
  const session = getAccountSession();
  if (!entry || !session) return;
  const { activatingEntryIds } = session.state;
  if (isMobile.value && entry.kind !== "text") {
    await saveEntry(entry);
    return;
  }
  if (activatingEntryIds.value.has(entry.id)) {
    // 下载中再次激活 = 取消该下载；无下载（如纯文本快速粘贴）则维持防重复。
    if (await cancelEntryDownloads(entry.id) > 0) showToast("已取消下载", "info");
    return;
  }
  activatingEntryIds.value.add(entry.id);
  try {
    // Wait for all missing content before copying or pasting the entry.
    await downloadEntry(entry.id, session);
    // 串行化：等待轮到自己再写剪贴板，避免并发 paste_entry 交错。
    const write = session.state.clipboardWriteChain.then(() => session.invoke(command, { entryId: entry.id }));
    session.state.clipboardWriteChain = write.catch(() => undefined);
    await write;
    // 粘贴的结果用户肉眼可见（内容已进入目标应用），不再弹提示；复制的结果
    // 看不见，保留确认提示。
    if (command === "copy_entry") showToast("已复制到系统剪贴板", "success");
  } catch (error) {
    // 取消的提示已由取消方给出，原激活方静默收尾。
    if (session.signal.aborted || error instanceof DownloadCancelledError) return;
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
  const session = getAccountSession();
  if (!session) return;
  const { savingEntryId } = session.state;
  if (savingEntryId.value === entry.id) {
    if (await cancelEntryDownloads(entry.id) > 0) showToast("已取消下载", "info");
    return;
  }
  if (savingEntryId.value || !canSaveEntry(entry)) return;
  savingEntryId.value = entry.id;
  let saveId: string | undefined;
  try {
    if (!platformCapabilities.value.nativeFileExport) {
      await downloadEntry(entry.id, session);
      showToast("内容已下载到应用缓存，可在 ClipRoam 中离线使用", "success");
    } else {
      await session.start(() => session.ready);
      const preparation = await session.acquire(nativeInvoke<SavePreparation | null>("prepare_save_entry", {
        entryId: entry.id,
        sessionId: session.sessionId,
      }), (value) => { if (value) void cancelSave(value.saveId); });
      if (!preparation) return;
      saveId = preparation.saveId;

      await downloadEntry(entry.id, session);

      const saved = await session.invoke<number>("finish_save_entry", { saveId: preparation.saveId });
      saveId = undefined;
      if (isMobile.value) showToast("已保存到所选目录", "success");
      else if (saved > 0) showToast(`已保存 ${saved} 个文件`, "success");
    }
  } catch (error) {
    if (saveId) await cancelSave(saveId);
    if (session.signal.aborted || error instanceof DownloadCancelledError) return;
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

// Server-confirmed deletion only marks history changed; the view owns refetching.
export async function removeEntry(entry: ClipboardEntry): Promise<void> {
  const client = getSyncClient();
  if (!client) {
    showToast("网络异常，暂时无法删除，请检查同步连接", "error");
    return;
  }
  try {
    await client.history.delete(entry.id);
    if (!client.session.signal.aborted) refreshHistory();
  } catch (error) {
    if (!client.session.signal.aborted) showToast(`删除失败：${errorMessage(error)}`, "error");
    return;
  }
}
