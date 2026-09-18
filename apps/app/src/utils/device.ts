import { invoke } from "@tauri-apps/api/core";
import type { Device } from "../types";

type DeviceInfo = { deviceName: string; cpu: string; osType: string; osVersion: string; appVersion: string };
type RustDeviceIdentity = { deviceId: string; deviceAlias: string | null };

export type DeviceIdentity = {
  deviceId: string;
  /** 用户设置的别名；空串表示未设置。 */
  deviceAlias: string;
  systemDeviceName: string;
  osType: string;
  osVersion: string;
  appVersion: string;
};

export async function getDeviceIdentity(): Promise<DeviceIdentity> {
  // id 与别名读 device.json；展示信息每次从系统取，机器改名后立即生效。
  const [identity, info] = await Promise.all([
    invoke<RustDeviceIdentity>("get_device_identity"),
    invoke<DeviceInfo>("get_device_info"),
  ]);
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
