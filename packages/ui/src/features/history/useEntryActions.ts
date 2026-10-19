import { errorMessage, canSaveEntry } from "../../utils";
import { accountStateRef, getAccountSession } from "../../accountSession";
import { isMobile, isQuickPaste, usePlatform, showNotification } from "../../platform/runtime";
import { downloadEntry } from "../downloads/useDownloads";
import { DownloadCancelledError } from "../downloads/downloadManager";
import { refreshHistory } from "./useHistorySync";
import type { LocalClipboardEntry } from "../../types";

/**
 * 条目动作（从 App.vue 下沉）：复制/粘贴/另存/删除。文件准备经下载模块，历史失效走 refreshHistory。
 */
export const activatingEntryIds = accountStateRef("activatingEntryIds");
export const savingEntryId = accountStateRef("savingEntryId");
const { platformCapabilities } = usePlatform();
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
  if (activatingEntryIds.value.has(entry.id)) return;
  activatingEntryIds.value.add(entry.id);
  try {
    // Wait for all missing content before copying or pasting the entry.
    await downloadEntry(entry.id, session);
    // 串行化：等待轮到自己再写剪贴板，避免并发 paste_entry 交错。
    const write = session.state.clipboardWriteChain.then(() => session.capture.writeEntry(entry.id, command === "paste_entry"));
    session.state.clipboardWriteChain = write.catch(() => undefined);
    await write;
    // 粘贴的结果用户肉眼可见（内容已进入目标应用），不再弹提示；复制的结果
    // 看不见，保留确认提示。
    if (command === "copy_entry") showNotification({ message: "已复制到系统剪贴板", tone: "success" });
  } catch (error) {
    // 取消的提示已由取消方给出，原激活方静默收尾。
    if (session.signal.aborted || error instanceof DownloadCancelledError) return;
    if (String(error).includes("clipboard entry was not found")) {
      refreshHistory();
      return;
    }
    showNotification({ message: String(error), tone: "error" });
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
  if (savingEntryId.value || !canSaveEntry(entry)) return;
  savingEntryId.value = entry.id;
  let saveId: string | undefined;
  try {
    if (!platformCapabilities.value.nativeFileExport) {
      await downloadEntry(entry.id, session);
      showNotification({ message: "内容已下载到应用缓存，可在 随心贴 中离线使用", tone: "success" });
    } else {
      await session.start(() => session.ready);
      const preparation = await session.acquire(session.downloads.prepareSave(entry.id), (value) => {
        if (value) void session.downloads.cancelSave(value.saveId);
      });
      if (!preparation) return;
      saveId = preparation.saveId;

      await downloadEntry(entry.id, session, preparation.saveId);

      const saved = await session.downloads.finishSave(preparation.saveId);
      saveId = undefined;
      if (isMobile.value) showNotification({ message: "已保存到所选目录", tone: "success" });
      else if (saved > 0) showNotification({ message: `已保存 ${saved} 个文件`, tone: "success" });
    }
  } catch (error) {
    if (saveId) await session.downloads.cancelSave(saveId);
    if (session.signal.aborted || error instanceof DownloadCancelledError) return;
    showNotification({ message: `${isMobile.value ? "下载" : "另存为"}失败：${errorMessage(error)}`, tone: "error" });
  } finally {
    savingEntryId.value = "";
  }
}

/**
 * 快速粘贴模式单击条目触发粘贴；主界面预览或菜单操作触发复制。
 */
export function activateFromView(entry: LocalClipboardEntry, viaClick: boolean): void {
  if (viaClick) {
    if (isQuickPaste.value) void pasteEntry(entry);
    else if (isMobile.value) void copyEntry(entry);
    return;
  }
  if (isQuickPaste.value) void pasteEntry(entry);
  else if (entry.kind === "files") {
    showNotification({ message: "文件请使用 Ctrl+Shift+V 快捷粘贴，或点击“下载”手动下载", tone: "info" });
  } else {
    void copyEntry(entry);
  }
}

export async function setEntriesFavorite(entryIds: readonly string[], isFavorite: boolean): Promise<boolean> {
  if (!entryIds.length) return true;
  const session = getAccountSession();
  if (!session?.client) {
    showNotification({ message: "网络异常，暂时无法修改收藏，请检查同步连接", tone: "error" });
    return false;
  }
  try {
    await session.history.setFavorite(entryIds, isFavorite);
    return !session.signal.aborted;
  } catch (error) {
    if (!session.signal.aborted) showNotification({ message: `修改收藏失败：${errorMessage(error)}`, tone: "error" });
    return false;
  }
}

// Server-confirmed deletion only marks history changed; the view owns refetching.
export async function removeEntries(entryIds: readonly string[]): Promise<boolean> {
  if (!entryIds.length) return true;
  const session = getAccountSession();
  if (!session?.client) {
    showNotification({ message: "网络异常，暂时无法删除，请检查同步连接", tone: "error" });
    return false;
  }
  try {
    await session.history.delete(entryIds);
    return !session.signal.aborted;
  } catch (error) {
    if (!session.signal.aborted) showNotification({ message: `删除失败：${errorMessage(error)}`, tone: "error" });
    return false;
  }
}
