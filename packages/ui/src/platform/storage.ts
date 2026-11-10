import { applicationDatabase } from "./database";
import { native, type DeviceInfo } from "./native";
import type { Device } from "@cliproam/protocol";
import type { SyncConfig } from "../types";
import { normalizeServerAddress } from "../features/sync/authApi";

/** Application storage for login configuration and device identity. */
const CONFIG_KEY = "cliproam.lastlogin";
const SAVED_ACCOUNTS_KEY = "cliproam.saved-accounts";
type SavedAccounts = Record<string, Record<string, string>>;

function serverKey(config: SyncConfig): string {
  return normalizeServerAddress(config.serverAddress);
}

export async function readSavedAccounts(): Promise<SyncConfig[]> {
  const accounts = await readSavedAccountMap();
  return Object.entries(accounts).flatMap(([address, users]) => {
    return Object.entries(users).map(([username, token]) => ({
      serverAddress: normalizeServerAddress(address),
      username,
      sessionToken: token,
    }));
  });
}

async function readSavedAccountMap(): Promise<SavedAccounts> {
  let accounts = await applicationDatabase.get<SavedAccounts>("meta", SAVED_ACCOUNTS_KEY);
  const legacy = await applicationDatabase.get<SavedAccounts>("meta", "cliproam.saved-accounts.v2");
  if (legacy) {
    accounts ??= "ciphertext" in legacy ? {} : legacy;
    await applicationDatabase.batch([
      { name: "meta", id: SAVED_ACCOUNTS_KEY, value: accounts },
      { name: "meta", id: "cliproam.saved-accounts.v2" },
    ]);
  }
  return accounts && !("ciphertext" in accounts) ? accounts : {};
}

/** Forget credentials without deleting the account's clipboard archive or stopping its live session. */
export async function removeSavedAccounts(config?: SyncConfig, wholeServer = false): Promise<void> {
  const accounts = config ? await readSavedAccountMap() : {};
  if (config) {
    const key = serverKey(config);
    if (wholeServer) delete accounts[key];
    else if (Object.prototype.hasOwnProperty.call(accounts, key)) {
      delete accounts[key]![config.username];
      if (!Object.keys(accounts[key]!).length) delete accounts[key];
    }
  }
  await readLoginConfig();
  await applicationDatabase.batch([
    { name: "meta", id: SAVED_ACCOUNTS_KEY, value: accounts },
    { name: "meta", id: "cliproam.saved-accounts.v2" },
    { name: "meta", id: "cliproam.storage-key.v1" },
  ]);
}

export async function readLoginConfig(): Promise<SyncConfig | null> {
  let config = await applicationDatabase.get<{ address: string; username: string }>("meta", CONFIG_KEY);
  const legacy = await applicationDatabase.get<SyncConfig>("meta", "cliproam.connection.v2");
  if (!config && legacy && typeof legacy.serverAddress === "string" && typeof legacy.username === "string") {
    config = { address: normalizeServerAddress(legacy.serverAddress), username: legacy.username };
  }
  if (legacy) {
    await applicationDatabase.batch([
      ...(config ? [{ name: "meta" as const, id: CONFIG_KEY, value: config }] : []),
      { name: "meta", id: "cliproam.connection.v2" },
      { name: "meta", id: "cliproam.storage-key.v1" },
    ]);
  }
  if (!config) return null;
  const serverAddress = normalizeServerAddress(config.address);
  const accounts = await readSavedAccountMap();
  const users = accounts[serverAddress];
  const sessionToken = users && Object.prototype.hasOwnProperty.call(users, config.username) ? users[config.username] : "";
  return { serverAddress, username: config.username, sessionToken };
}

export async function writeLoginConfig(config: SyncConfig | null): Promise<void> {
  if (!config) {
    await applicationDatabase.batch([
      { name: "meta", id: CONFIG_KEY },
      { name: "meta", id: "cliproam.connection.v2" },
      { name: "meta", id: "cliproam.storage-key.v1" },
    ]);
    return;
  }
  config = { ...config, serverAddress: normalizeServerAddress(config.serverAddress) };
  const accounts = await readSavedAccountMap();
  const key = serverKey(config);
  const users = Object.prototype.hasOwnProperty.call(accounts, key) ? accounts[key]! : {};
  if (config.sessionToken || Object.prototype.hasOwnProperty.call(users, config.username)) {
    accounts[key] = { [config.username]: config.sessionToken, ...Object.fromEntries(Object.entries(users).filter(([username]) => username !== config.username)) };
  }
  await applicationDatabase.batch([
    { name: "meta", id: CONFIG_KEY, value: { address: config.serverAddress, username: config.username } },
    { name: "meta", id: "cliproam.connection.v2" },
    { name: "meta", id: SAVED_ACCOUNTS_KEY, value: accounts },
    { name: "meta", id: "cliproam.saved-accounts.v2" },
    { name: "meta", id: "cliproam.storage-key.v1" },
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
