import { NativeWeb } from "@cliproam/ui/native-web";
import type { ClipboardData, ClipboardWrite, DeviceInfo, DirectoryEntry, PlatformCapabilities, SaveTarget } from "@cliproam/ui/native";
import { bytesToBase64 } from "@cliproam/ui/bytes";
import { request } from "./bridge";
import packageInfo from "../../package.json";

export class NativeVscode extends NativeWeb {
  async getAppVersion(): Promise<string> { return packageInfo.version; }
  async getDeviceInfo(): Promise<DeviceInfo> {
    return { ...await super.getDeviceInfo(), ...await request<Partial<DeviceInfo>>("device.info"), appVersion: packageInfo.version };
  }
  async getCapabilities(): Promise<PlatformCapabilities> {
    return { ...await super.getCapabilities(), imageClipboard: false, nativeFileExport: true };
  }
  async selectSaveTarget(single: boolean, fileName: string): Promise<SaveTarget | null> {
    return request("files.selectSaveTarget", { single, fileName });
  }
  async exportDirectory(target: SaveTarget, entries: AsyncIterable<DirectoryEntry>, signal: AbortSignal): Promise<number> {
    if (target.kind !== "path") throw new Error("未选择保存位置");
    let count = 0;
    try {
      for await (const entry of entries) {
        signal.throwIfAborted();
        if (!entry.stream) {
          await request("files.createDirectory", { target: target.value, path: entry.path });
          continue;
        }
        const reader = entry.stream.getReader();
        try {
          await request("files.write", { target: target.value, path: entry.path, data: "", append: false });
          while (true) {
            signal.throwIfAborted();
            const { done, value } = await reader.read();
            if (done) break;
            signal.throwIfAborted();
            await request("files.write", { target: target.value, path: entry.path, data: bytesToBase64(value), append: true });
          }
          signal.throwIfAborted();
          count++;
        } finally {
          await reader.cancel().catch(() => undefined);
          reader.releaseLock();
        }
      }
      return count;
    } finally {
      await request("files.release", target.value);
    }
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
