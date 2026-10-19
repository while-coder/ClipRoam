import type { AccountSession } from "../../accountSession";
import type { ShareImportSummary } from "../../types";
import { shareReceiver } from "../../platform/share";
import { profileName } from "../../utils";

/** Keep inbox items until every part is imported; stable capture IDs deduplicate retries. */
export async function importPendingShares(session: AccountSession, reportError: (message: string) => void): Promise<ShareImportSummary> {
  const result = { shares: 0, texts: 0, images: 0, files: 0 };
  const shares = await shareReceiver.pending(profileName(session.accountKey));
  for (const share of shares) {
    session.signal.throwIfAborted();
    try {
      if (share.text) {
        await session.capture.capture({ kind: "text", text: share.text, html: share.html }, `${share.id}:text`);
        result.texts++;
      }
      for (let i = 0; i < share.images.length; i++) {
        const image = await session.files.readImage(share.images[i]!);
        await session.capture.capture(image, `${share.id}:image:${i}`);
        result.images++;
      }
      if (share.paths.length) {
        await session.capture.capture({ kind: "files", paths: share.paths }, `${share.id}:files`);
        result.files += share.paths.length;
      }
      if (share.files?.length) {
        await session.capture.captureFiles(share.files, `${share.id}:browser-files`);
        result.files += share.files.length;
      }
      result.shares++;
    } catch (error) {
      session.signal.throwIfAborted();
      reportError(`分享导入失败：${String(error)}`);
      continue;
    }
    session.signal.throwIfAborted();
    await shareReceiver.acknowledge(share.id);
  }
  return result;
}
