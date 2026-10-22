import type { ShareReceiver, SharedContent } from "@cliproam/ui/share";
import type { Unlisten } from "@cliproam/ui/native";
import { request } from "./bridge";
import { base64ToBytes } from "@cliproam/ui/bytes";

/** Treat context-menu text and files as incoming VS Code shares. */
export class VscodeShareReceiver implements ShareReceiver {
  async isSupported(): Promise<boolean> { return true; }
  async listen(handler: () => void): Promise<Unlisten> {
    const listener = ({ data }: MessageEvent): void => {
      if (data?.type === "sharesChanged") handler();
    };
    window.addEventListener("message", listener);
    return () => window.removeEventListener("message", listener);
  }
  async pending(scope: string): Promise<SharedContent[]> {
    const shares = await request<SharedContent[]>("shares.pending", scope);
    for (const share of shares) {
      share.files = [];
      for (const path of share.paths) {
        const file = await request<{ name: string; lastModified: number; data: string }>("shares.readFile", { id: share.id, path });
        share.files.push(new File([base64ToBytes(file.data)], file.name, { lastModified: file.lastModified }));
      }
      share.paths = [];
    }
    return shares;
  }
  async acknowledge(id: string): Promise<void> { await request("shares.acknowledge", id); }
}
