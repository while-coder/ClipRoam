import { base64ToBytes, bytesToBase64 } from "../utils";
import { applicationDatabase } from "./database";
import { native, type DeviceInfo } from "./native";
import type { Device } from "@cliproam/protocol";
import type { SyncConfig } from "../types";

/** Application storage for encrypted login configuration and device identity. */
const CONFIG_KEY = "cliproam.connection.v1";
const SAVED_ACCOUNTS_KEY = "cliproam.saved-accounts.v1";
const ENCRYPTION_KEY = "cliproam.storage-key.v1";
type EncryptedConfig = { version: 1; iv: string; ciphertext: string };
type SavedAccounts = Record<string, { username: string; token: string }[]>;

function serverKey(config: SyncConfig): string {
  return `${config.serverAddress}:${config.serverProtocol === "http"}`;
}

async function readEncrypted<T>(id: string): Promise<T | undefined> {
  const stored = await applicationDatabase.get<EncryptedConfig>("meta", id);
  if (!stored) return undefined;
  if (stored.version !== 1) throw new Error("不支持的本地登录配置版本");
  const key = await localKey(false);
  const plaintext = await crypto.subtle.decrypt({ name: "AES-GCM", iv: base64ToBytes(stored.iv) }, key, base64ToBytes(stored.ciphertext));
  return JSON.parse(new TextDecoder().decode(plaintext)) as T;
}

async function encrypt(value: unknown): Promise<EncryptedConfig> {
  const key = await localKey(true), iv = crypto.getRandomValues(new Uint8Array(12));
  const plaintext = new TextEncoder().encode(JSON.stringify(value));
  const ciphertext = await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, plaintext);
  return { version: 1, iv: bytesToBase64(iv), ciphertext: bytesToBase64(new Uint8Array(ciphertext)) };
}

export async function readSavedAccounts(): Promise<SyncConfig[]> {
  const accounts = await readEncrypted<SavedAccounts>(SAVED_ACCOUNTS_KEY) ?? {};
  return Object.entries(accounts).flatMap(([key, users]) => {
    const separator = key.lastIndexOf(":");
    return users.map(({ username, token }) => ({
      serverAddress: key.slice(0, separator),
      serverProtocol: key.slice(separator + 1) === "true" ? "http" as const : "https" as const,
      username,
      sessionToken: token,
    }));
  });
}

/** Forget credentials without deleting the account's clipboard archive or stopping its live session. */
export async function removeSavedAccounts(config?: SyncConfig, wholeServer = false): Promise<void> {
  const accounts = config ? await readEncrypted<SavedAccounts>(SAVED_ACCOUNTS_KEY) ?? {} : {};
  if (config) {
    const key = serverKey(config);
    const users = wholeServer ? [] : accounts[key]?.filter((user) => user.username !== config.username) ?? [];
    if (users.length) accounts[key] = users;
    else delete accounts[key];
  }
  const current = await readLoginConfig();
  const changes = [{ name: "meta" as const, id: SAVED_ACCOUNTS_KEY, value: await encrypt(accounts) }];
  if (current && (!config || (serverKey(current) === serverKey(config) && (wholeServer || current.username === config.username)))) {
    changes.push({ name: "meta", id: CONFIG_KEY, value: await encrypt({ ...current, sessionToken: "" }) });
  }
  await applicationDatabase.batch(changes);
}

async function localKey(create: boolean): Promise<CryptoKey> {
  let encoded = await applicationDatabase.get<string>("meta", ENCRYPTION_KEY);
  if (encoded) {
    let key: CryptoKey | undefined;
    try {
      const bytes = base64ToBytes(encoded);
      if (bytes.length !== 32) throw new Error("invalid key");
      key = await crypto.subtle.importKey("raw", bytes, "AES-GCM", false, ["encrypt", "decrypt"]);
    } catch {
      if (!create) throw new Error("本地登录配置无法解密，请重新登录");
      encoded = undefined;
    }
    if (key && encoded) {
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
  const stored = await applicationDatabase.get<EncryptedConfig>("meta", CONFIG_KEY);
  if (!stored) return null;
  try {
    if (stored.version !== 1) throw new Error("unsupported version");
    const key = await localKey(false);
    const plaintext = await crypto.subtle.decrypt({name:"AES-GCM",iv:base64ToBytes(stored.iv)},key,base64ToBytes(stored.ciphertext));
    const config = JSON.parse(new TextDecoder().decode(plaintext)) as SyncConfig;
    if (typeof config.serverAddress !== "string" || typeof config.username !== "string" || typeof config.sessionToken !== "string"
      || (config.serverProtocol !== "http" && config.serverProtocol !== "https")) throw new Error("invalid config");
    return config;
  } catch {
    throw new Error("本地登录配置无法解密，请输入账号信息重新登录");
  }
}

export async function writeLoginConfig(config: SyncConfig | null): Promise<void> {
  if (!config) {
    await applicationDatabase.delete("meta", CONFIG_KEY);
    return;
  }
  const accounts = await readEncrypted<SavedAccounts>(SAVED_ACCOUNTS_KEY) ?? {};
  const key = serverKey(config);
  const users = accounts[key] ?? [];
  const existing = users.findIndex((user) => user.username === config.username);
  if (config.sessionToken || existing >= 0) {
    accounts[key] = [{ username: config.username, token: config.sessionToken }, ...users.filter((user) => user.username !== config.username)];
  }
  await applicationDatabase.batch([
    { name: "meta", id: CONFIG_KEY, value: await encrypt(config) },
    { name: "meta", id: SAVED_ACCOUNTS_KEY, value: await encrypt(accounts) },
  ]);
}

type DeviceIdentity = { deviceId: string; deviceInfo: DeviceInfo };

export async function getDeviceIdentity(): Promise<DeviceIdentity> {
  const info = await native.getDeviceInfo();
  let identity: DeviceIdentity;
  // 旧配置的顶层别名在读取时收进 deviceInfo，之后只保存统一结构。
  await applicationDatabase.update<DeviceIdentity & { deviceAlias?: string | null }>("meta", "device", (stored) => {
    identity = {
      deviceId: stored?.deviceId ?? crypto.randomUUID(),
      deviceInfo: { ...info, deviceAlias: stored?.deviceInfo?.deviceAlias ?? stored?.deviceAlias?.trim() ?? "" },
    };
    return identity;
  });
  return identity!;
}

export async function getDevice(): Promise<Device> {
  const identity = await getDeviceIdentity();
  return { id: identity.deviceId, deviceInfo: JSON.stringify(identity.deviceInfo) };
}

export async function saveDeviceAlias(alias: string): Promise<void> {
  const value = alias.trim();
  if ([...value].length > 80) throw new Error("设备别名不能超过 80 个字符");
  await getDeviceIdentity();
  await applicationDatabase.update<DeviceIdentity>("meta", "device", (identity) => identity
    ? { ...identity, deviceInfo: { ...identity.deviceInfo, deviceAlias: value } }
    : undefined);
}
