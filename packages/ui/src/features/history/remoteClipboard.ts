import { clientSessionManager } from "../sync/clientSessionManager";
import { getAccountSession } from "../../core/accountSession";
import type { ClipboardManifestEntry } from "@cliproam/protocol";
import { isMobile, usePlatform } from "../../platform/runtime";
import { showToast } from "../../core/toast";
import { errorMessage } from "../../utils/error";
import { ensureLocalFiles } from "../downloads/useDownloads";

const { platformCapabilities } = usePlatform();

/** 本机捕获落库计数；远端激活靠它识别「用户刚复制过」的竞态。 */
export function bumpLocalClipboardRevision(): void {
  const session = getAccountSession();
  if (session) session.state.localClipboardRevision += 1;
}

export async function activateRemoteClipboard(manifestEntry: ClipboardManifestEntry): Promise<void> {
  const session = getAccountSession();
  const config = clientSessionManager.getActiveConfig();
  const client = clientSessionManager.getConnection();
  if (
    isMobile.value
    || !platformCapabilities.value.automaticPaste
    || !clientSessionManager.getActivePreferences().autoReceiveClipboard
    || !session
    || !client
  ) return;

  const activationRevision = ++session.state.remoteActivationRevision;
  const startingLocalRevision = session.state.localClipboardRevision;
  try {
    // Push supplies the same identity/revision pair as a manifest page.
    const [entry] = await session.history.fetchHistoryInfo([manifestEntry]);
    if (!entry || entry.kind === "files") return;
    let localEntry = entry;
    if (clientSessionManager.getActiveConfig() !== config || !clientSessionManager.getActivePreferences().autoReceiveClipboard) return;
    if (entry.kind === "image") localEntry = await ensureLocalFiles(localEntry, session);

    // A newer remote activation or a real local copy wins while an image is
    // downloading; never replace content the user copied in the meantime.
    if (
      activationRevision !== session.state.remoteActivationRevision
      || startingLocalRevision !== session.state.localClipboardRevision
      || clientSessionManager.getActiveConfig() !== config
      || !clientSessionManager.getActivePreferences().autoReceiveClipboard
    ) return;
    await session.capture.writeEntry(localEntry.id);
  } catch (error) {
    if (
      !session.signal.aborted
      && activationRevision === session.state.remoteActivationRevision
      && clientSessionManager.getActiveConfig() === config
      && clientSessionManager.getActivePreferences().autoReceiveClipboard
    ) {
      showToast(`自动接收剪贴板失败：${errorMessage(error)}`, "error");
    }
  }
}

