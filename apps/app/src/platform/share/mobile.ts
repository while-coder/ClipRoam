import { addPluginListener, invoke } from "@tauri-apps/api/core";
import type { ShareReceiver, SharedContent, ShareReceiverEvent } from "@cliproam/ui/share";
import type { Unlisten } from "@cliproam/ui/native";

/** Android share intents and the iOS Share Extension use the same native inbox. */
export class MobileShareReceiver implements ShareReceiver {
  async isSupported(): Promise<boolean> {
    return (await invoke<{ shareReceiver: boolean }>("native_get_platform_capabilities")).shareReceiver;
  }
  async listen(handler: (event: ShareReceiverEvent) => void): Promise<Unlisten> {
    const listener = await addPluginListener("cliproam-share-receiver", "received", handler);
    return () => { void listener.unregister(); };
  }
  pending(scope: string): Promise<SharedContent[]> { return invoke("native_pending_shares", { scope }); }
  acknowledge(id: string): Promise<void> { return invoke("native_ack_share", { id }); }
}
