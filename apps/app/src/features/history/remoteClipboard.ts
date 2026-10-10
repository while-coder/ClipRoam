import { invoke } from "@tauri-apps/api/core";
import { isMobile } from "../../composables/usePlatform";
import { showToast } from "../toast/useToast";
import { errorMessage } from "../../utils/error";
import { getSyncClient } from "../sync/syncEngine";
import { getActiveConfig, getActivePreferences } from "../sync/syncSession";
import { ensurePasteReady } from "../downloads/useDownloads";

let localClipboardRevision = 0;
let remoteActivationRevision = 0;

/** 本机捕获落库计数；远端激活靠它识别「用户刚复制过」的竞态。 */
export function bumpLocalClipboardRevision(): void {
  localClipboardRevision += 1;
}

export async function activateRemoteClipboard(entryId: string): Promise<void> {
  const config = getActiveConfig();
  const client = getSyncClient();
  if (
    isMobile.value
    || !getActivePreferences().autoReceiveClipboard
    || !client
  ) return;

  const activationRevision = ++remoteActivationRevision;
  const startingLocalRevision = localClipboardRevision;
  try {
    // Push carries an identity only; the same detail backfill supplies the cache.
    const [entry] = await client.history.fetchHistoryInfo([entryId]);
    if (!entry || entry.kind === "files") return;
    let localEntry = entry;
    if (getActiveConfig() !== config || !getActivePreferences().autoReceiveClipboard) return;
    if (entry.kind === "image") localEntry = await ensurePasteReady(localEntry);

    // A newer remote activation or a real local copy wins while an image is
    // downloading; never replace content the user copied in the meantime.
    if (
      activationRevision !== remoteActivationRevision
      || startingLocalRevision !== localClipboardRevision
      || getActiveConfig() !== config
      || !getActivePreferences().autoReceiveClipboard
    ) return;
    await invoke("activate_remote_entry", { entryId: localEntry.id });
  } catch (error) {
    if (
      activationRevision === remoteActivationRevision
      && getActiveConfig() === config
      && getActivePreferences().autoReceiveClipboard
    ) {
      showToast(`自动接收剪贴板失败：${errorMessage(error)}`, "error");
    }
  }
}

