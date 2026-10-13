import { applicationDatabase } from "../core/database";
import { native } from "./native";
import type { Device, SyncConfig } from "../types";
import { base64ToBytes, bytesToBase64 } from "../utils/bytes";

/** Application storage for encrypted login configuration and device identity. */
const CONFIG_KEY = "cliproam.connection.v1";
const ENCRYPTION_KEY = "cliproam.storage-key.v1";
type EncryptedConfig = { version: 1; iv: string; ciphertext: string };

async function localKey(create: boolean): Promise<CryptoKey> {
  let encoded = await applicationDatabase.get<string>("meta", ENCRYPTION_KEY)
    ?? localStorage.getItem(ENCRYPTION_KEY);
  if (encoded) {
    let key: CryptoKey | undefined;
    try {
      const bytes = base64ToBytes(encoded);
      if (bytes.length !== 32) throw new Error("invalid key");
      key = await crypto.subtle.importKey("raw", bytes, "AES-GCM", false, ["encrypt", "decrypt"]);
    } catch {
      if (!create) throw new Error("本地登录配置无法解密，请重新登录");
      encoded = null;
    }
    if (key && encoded) {
      await applicationDatabase.put("meta", ENCRYPTION_KEY, encoded);
      return key;
    }
  }
  if (!encoded && create) {
    encoded = bytesToBase64(crypto.getRandomValues(new Uint8Array(32)));
    await applicationDatabase.put("meta", ENCRYPTION_KEY, encoded);
  }
  if (!encoded) throw new Error("本地登录配置无法解密，请重新登录");
  return crypto.subtle.importKey("raw",base64ToBytes(encoded),"AES-GCM",false,["encrypt","decrypt"]);
}

/** Persist the local key and ciphertext alongside device identity in IndexedDB. */
export async function readLoginConfig(): Promise<SyncConfig | null> {
  let stored = await applicationDatabase.get<EncryptedConfig>("meta", CONFIG_KEY);
  const legacy = stored ? null : localStorage.getItem(CONFIG_KEY);
  if (!stored && !legacy) return null;
  try {
    stored ??= JSON.parse(legacy!) as EncryptedConfig;
    if (stored.version !== 1) throw new Error("unsupported version");
    const key = await localKey(false);
    const plaintext = await crypto.subtle.decrypt({name:"AES-GCM",iv:base64ToBytes(stored.iv)},key,base64ToBytes(stored.ciphertext));
    const config = JSON.parse(new TextDecoder().decode(plaintext)) as SyncConfig;
    if (typeof config.serverAddress !== "string" || typeof config.username !== "string" || typeof config.sessionToken !== "string"
      || (config.serverProtocol !== "http" && config.serverProtocol !== "https")) throw new Error("invalid config");
    if (legacy) await applicationDatabase.put("meta", CONFIG_KEY, stored);
    localStorage.removeItem(CONFIG_KEY);
    localStorage.removeItem(ENCRYPTION_KEY);
    return config;
  } catch {
    throw new Error("本地登录配置无法解密，请输入账号信息重新登录");
  }
}

export async function writeLoginConfig(config: SyncConfig | null): Promise<void> {
  if (!config) {
    await applicationDatabase.delete("meta", CONFIG_KEY);
    localStorage.removeItem(CONFIG_KEY);
    return;
  }
  const key = await localKey(true), iv = crypto.getRandomValues(new Uint8Array(12));
  const plaintext = new TextEncoder().encode(JSON.stringify(config));
  const ciphertext = await crypto.subtle.encrypt({name:"AES-GCM",iv},key,plaintext);
  await applicationDatabase.put("meta", CONFIG_KEY, {version:1,iv:bytesToBase64(iv),ciphertext:bytesToBase64(new Uint8Array(ciphertext))} satisfies EncryptedConfig);
  localStorage.removeItem(CONFIG_KEY);
  localStorage.removeItem(ENCRYPTION_KEY);
}

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
  const info = await native.getDeviceInfo();
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
