import type { ShareReceiver, SharedContent, ShareReceiverEvent } from "@cliproam/ui/share";
import type { Unlisten } from "@cliproam/ui/native";
import { listShares, removeShare } from "./inbox";

/** Web Share Target inbox, including installed Android PWAs. */
export class PwaShareReceiver implements ShareReceiver {
  constructor(private readonly scope: string) {}
  async isSupported(): Promise<boolean> { return "serviceWorker" in navigator && window.isSecureContext; }
  async pending(_accountScope: string): Promise<SharedContent[]> {
    return (await listShares(this.scope)).map((share) => ({ ...share, images: [], paths: [] }));
  }
  async acknowledge(id: string): Promise<void> { await removeShare(this.scope, id); }
  async listen(handler: (event: ShareReceiverEvent) => void): Promise<Unlisten> {
    const focus = () => handler({});
    window.addEventListener("focus", focus);
    return () => window.removeEventListener("focus", focus);
  }
}
