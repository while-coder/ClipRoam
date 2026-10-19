import { NativeWeb } from "@cliproam/ui/native-web";
import type { DeviceInfo } from "@cliproam/ui/native";
import packageInfo from "../package.json";

/** PWA adapter; platform-specific APIs can override the shared Web implementation here. */
export class NativePwa extends NativeWeb {
  async getAppVersion(): Promise<string> { return packageInfo.version; }
  async getDeviceInfo(): Promise<DeviceInfo> {
    return {...await super.getDeviceInfo(),deviceName:"ClipRoam PWA",appVersion:packageInfo.version};
  }
}
