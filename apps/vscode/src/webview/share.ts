import type { ShareReceiver, SharedContent } from "@cliproam/ui/share";
import type { Unlisten } from "@cliproam/ui/native";
import { request } from "./bridge";

/** Treat editor context-menu selections as incoming VS Code shares. */
export class VscodeShareReceiver implements ShareReceiver {
  async isSupported(): Promise<boolean> { return true; }
  async listen(handler: () => void): Promise<Unlisten> {
    const listener = ({ data }: MessageEvent): void => {
      if (data?.type === "sharesChanged") handler();
    };
    window.addEventListener("message", listener);
    return () => window.removeEventListener("message", listener);
  }
  pending(scope: string): Promise<SharedContent[]> { return request("shares.pending", scope); }
  async acknowledge(id: string): Promise<void> { await request("shares.acknowledge", id); }
}
