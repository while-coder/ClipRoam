import { applicationDatabase } from "../core/database";
import { isNative, nativeCall } from "../platform/runtime";
import type { Device } from "../types";

type DeviceInfo = { deviceName: string; cpu: string; osType: string; osVersion: string; appVersion: string };
type StoredDeviceIdentity = { deviceId: string; deviceAlias: string | null };

type DeviceIdentity = {
  deviceId: string;
  /** 用户设置的别名；空串表示未设置。 */
  deviceAlias: string;
  systemDeviceName: string;
  osType: string;
  osVersion: string;
  appVersion: string;
};

export async function getDeviceIdentity(): Promise<DeviceIdentity> {
  // Identity/alias are persisted in IndexedDB; native OS display information stays live.
  let identity = await applicationDatabase.get<StoredDeviceIdentity | null>("meta", "device");
  if (!identity) {
    identity = { deviceId: crypto.randomUUID(), deviceAlias: null };
    await applicationDatabase.put("meta", "device", identity);
  }
  const info: DeviceInfo = isNative ? await nativeCall<DeviceInfo>("native_get_device_info") : {
    deviceName: "浏览器", cpu: "", osType: "browser", osVersion: navigator.userAgent, appVersion: "web",
  };
  return {
    deviceId: identity.deviceId,
    deviceAlias: identity.deviceAlias?.trim() ?? "",
    systemDeviceName: info.deviceName,
    osType: info.osType,
    osVersion: info.osVersion,
    appVersion: info.appVersion,
  };
}

export async function getDevice(): Promise<Device> {
  const identity = await getDeviceIdentity();
  return {
    id: identity.deviceId,
    name: identity.deviceAlias || identity.systemDeviceName,
    platform: identity.osType,
    osVersion: identity.osVersion,
    appVersion: identity.appVersion,
  };
}

export async function saveDeviceAlias(alias: string): Promise<void> {
  const value = alias.trim();
  if ([...value].length > 80) throw new Error("设备别名不能超过 80 个字符");
  await getDeviceIdentity();
  await applicationDatabase.update<StoredDeviceIdentity | null>("meta", "device", (identity) => identity ? { ...identity, deviceAlias: value || null } : undefined);
}
