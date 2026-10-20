import { addPluginListener, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ShareReceiver, SharedContent } from "@cliproam/ui/share";
import type { Unlisten } from "@cliproam/ui/native";

/** Android share intents and the iOS Share Extension use the same native inbox. */
export class MobileShareReceiver implements ShareReceiver {
  async isSupported(): Promise<boolean> {
    return (await invoke<{ shareReceiver: boolean }>("native_get_platform_capabilities")).shareReceiver;
  }
  async listen(handler: () => void): Promise<Unlisten> {
    const stop = await listen("cliproam://share-received", handler);
    try {
      const listener = await addPluginListener<unknown>("cliproam-share-receiver", "share-received", handler);
      return () => { stop(); void listener.unregister(); };
    } catch (error) {
      stop();
      throw error;
    }
  }
  pending(scope: string): Promise<SharedContent[]> { return invoke("native_pending_shares", { scope }); }
  acknowledge(id: string): Promise<void> { return invoke("native_ack_share", { id }); }
}
