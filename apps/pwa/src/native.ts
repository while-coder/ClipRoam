import { NativeWeb } from "@cliproam/ui/native-web";
import type { DeviceInfo, PlatformCapabilities, SharedContent, ShareReceiverEvent, Unlisten } from "@cliproam/ui/native";
import { listShares, removeShare } from "./shareInbox";
import packageInfo from "../package.json";

/** PWA adapter; platform-specific APIs can override the shared Web implementation here. */
export class NativePwa extends NativeWeb {
  private readonly shareScope = new URL(import.meta.env.BASE_URL, window.location.href).href;
  async getCapabilities(): Promise<PlatformCapabilities> {
    return {...await super.getCapabilities(), shareReceiver: "serviceWorker" in navigator && window.isSecureContext};
  }
  async pendingShares(_scope: string): Promise<SharedContent[]> {
    return (await listShares(this.shareScope)).map((share) => ({...share, images:[], paths:[]}));
  }
  async acknowledgeShare(id: string): Promise<void> { await removeShare(this.shareScope, id); }
  async listenShares(handler: (event: ShareReceiverEvent) => void): Promise<Unlisten> {
    const focus = () => handler({});
    window.addEventListener("focus", focus);
    return () => window.removeEventListener("focus", focus);
  }
  async getAppVersion(): Promise<string> { return packageInfo.version; }
  async getDeviceInfo(): Promise<DeviceInfo> {
    return {...await super.getDeviceInfo(),deviceName:"随心贴 PWA",appVersion:packageInfo.version};
  }
}
