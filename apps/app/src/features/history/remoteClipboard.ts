import { getAccountSession } from "../sync/accountSession";
import type { ClipboardManifestEntry } from "@cliproam/protocol";
import { isMobile } from "../../composables/usePlatform";
import { showToast } from "../toast/useToast";
import { errorMessage } from "../../utils/error";
import { getSyncClient } from "../sync/syncEngine";
import { getActiveConfig, getActivePreferences } from "../sync/syncSession";
import { ensureLocalFiles } from "../downloads/useDownloads";

/** 本机捕获落库计数；远端激活靠它识别「用户刚复制过」的竞态。 */
export function bumpLocalClipboardRevision(): void {
  const session = getAccountSession();
  if (session) session.state.localClipboardRevision += 1;
}

export async function activateRemoteClipboard(manifestEntry: ClipboardManifestEntry): Promise<void> {
  const session = getAccountSession();
  const config = getActiveConfig();
  const client = getSyncClient();
  if (
    isMobile.value
    || !getActivePreferences().autoReceiveClipboard
    || !session
    || !client
  ) return;

  const activationRevision = ++session.state.remoteActivationRevision;
  const startingLocalRevision = session.state.localClipboardRevision;
  try {
    // Push supplies the same identity/revision pair as a manifest page.
    const [entry] = await client.history.fetchHistoryInfo([manifestEntry]);
    if (!entry || entry.kind === "files") return;
    let localEntry = entry;
    if (getActiveConfig() !== config || !getActivePreferences().autoReceiveClipboard) return;
    if (entry.kind === "image") localEntry = await ensureLocalFiles(localEntry, session);

    // A newer remote activation or a real local copy wins while an image is
    // downloading; never replace content the user copied in the meantime.
    if (
      activationRevision !== session.state.remoteActivationRevision
      || startingLocalRevision !== session.state.localClipboardRevision
      || getActiveConfig() !== config
      || !getActivePreferences().autoReceiveClipboard
    ) return;
    await session.invoke("activate_remote_entry", { entryId: localEntry.id });
  } catch (error) {
    if (
      !session.signal.aborted
      && activationRevision === session.state.remoteActivationRevision
      && getActiveConfig() === config
      && getActivePreferences().autoReceiveClipboard
    ) {
      showToast(`自动接收剪贴板失败：${errorMessage(error)}`, "error");
    }
  }
}

