import type { ShareReceiver, SharedContent } from "@cliproam/ui/share";
import type { Unlisten } from "@cliproam/ui/native";
import { listShares, removeShare } from "./inbox";

/** Web Share Target inbox, including installed Android PWAs. */
export class PwaShareReceiver implements ShareReceiver {
  constructor(private readonly scope: string) {}
  async isSupported(): Promise<boolean> { return "serviceWorker" in navigator && window.isSecureContext; }
  async pending(_accountScope: string): Promise<SharedContent[]> {
    return (await listShares(this.scope)).map((share) => ({ ...share, paths: [] }));
  }
  async acknowledge(id: string): Promise<void> { await removeShare(this.scope, id); }
  async listen(handler: () => void): Promise<Unlisten> {
    window.addEventListener("focus", handler);
    return () => window.removeEventListener("focus", handler);
  }
}
