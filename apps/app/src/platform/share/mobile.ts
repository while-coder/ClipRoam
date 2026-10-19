import { addPluginListener, invoke } from "@tauri-apps/api/core";
import type { ShareReceiver, SharedContent, ShareReceiverEvent, ShareUploadSession } from "@cliproam/ui/share";
import type { Unlisten } from "@cliproam/ui/native";

/** Android share intents and the iOS Share Extension use the same native inbox. */
export class MobileShareReceiver implements ShareReceiver {
  configureSession(session: ShareUploadSession | null): Promise<void> { return invoke("native_configure_share_session", { session }); }
  async isSupported(): Promise<boolean> {
    return (await invoke<{ shareReceiver: boolean }>("native_get_platform_capabilities")).shareReceiver;
  }
  async listen(handler: (event: ShareReceiverEvent) => void): Promise<Unlisten> {
    const listener = await addPluginListener<ShareReceiverEvent>("cliproam-share-receiver", "share-received", handler);
    return () => { void listener.unregister(); };
  }
  pending(scope: string): Promise<SharedContent[]> { return invoke("native_pending_shares", { scope }); }
  acknowledge(id: string): Promise<void> { return invoke("native_ack_share", { id }); }
}
