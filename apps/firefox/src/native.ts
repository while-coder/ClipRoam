import { NativeWeb } from "@cliproam/ui/native-web";
import type { DeviceInfo } from "@cliproam/ui/native";
import packageInfo from "../package.json";

/** Firefox extension adapter; shared Web APIs provide clipboard and file access. */
export class NativeFirefox extends NativeWeb {
  async getAppVersion(): Promise<string> { return packageInfo.version; }
  async getDeviceInfo(): Promise<DeviceInfo> {
    return {...await super.getDeviceInfo(),deviceName:"ClipRoam",appVersion:packageInfo.version};
  }
}
