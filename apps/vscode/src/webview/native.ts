import { NativeWeb } from "@cliproam/ui/native-web";
import type { ClipboardData, ClipboardWrite, DeviceInfo, PlatformCapabilities } from "@cliproam/ui/native";
import { request } from "./bridge";
import packageInfo from "../../package.json";

export class NativeVscode extends NativeWeb {
  async getAppVersion(): Promise<string> { return packageInfo.version; }
  async getDeviceInfo(): Promise<DeviceInfo> {
    return { ...await super.getDeviceInfo(), ...await request<Partial<DeviceInfo>>("device.info"), appVersion: packageInfo.version };
  }
  async getCapabilities(): Promise<PlatformCapabilities> {
    return { ...await super.getCapabilities(), imageClipboard: false, nativeFileExport: false };
  }
  async openExternalUrl(url: string): Promise<void> { await request("external.open", url); }
  async readClipboard(_textOnly: boolean): Promise<ClipboardData | null> {
    const text = await request<string>("clipboard.read");
    return text.trim() ? { kind: "text", text } : null;
  }
  async writeClipboard(payload: ClipboardWrite, _synthesize: boolean): Promise<void> {
    if (payload.kind !== "text") throw new Error("VS Code 扩展仅支持文本剪贴板；图片和文件可在历史中预览或下载到缓存");
    await request("clipboard.write", payload.text);
  }
}
