import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ShareReceiver, SharedContent, ShareReceiverEvent, ShareUploadSession } from "@cliproam/ui/share";
import type { Unlisten } from "@cliproam/ui/native";

/** Android share intents and the iOS Share Extension use the same native inbox. */
export class MobileShareReceiver implements ShareReceiver {
  configureSession(session: ShareUploadSession | null): Promise<void> { return invoke("native_configure_share_session", { session }); }
  async isSupported(): Promise<boolean> {
    return (await invoke<{ shareReceiver: boolean }>("native_get_platform_capabilities")).shareReceiver;
  }
  async listen(handler: (event: ShareReceiverEvent) => void): Promise<Unlisten> {
    return listen<ShareReceiverEvent>("cliproam://share-received", (event) => handler(event.payload));
  }
  pending(scope: string): Promise<SharedContent[]> { return invoke("native_pending_shares", { scope }); }
  acknowledge(id: string): Promise<void> { return invoke("native_ack_share", { id }); }
}
