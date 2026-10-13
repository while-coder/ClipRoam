import type { SyncConfig } from "../types";
import { base64ToBytes, bytesToBase64 } from "../utils/bytes";

const CONFIG_KEY = "cliproam.connection.v1";
const ENCRYPTION_KEY = "cliproam.storage-key.v1";
type EncryptedConfig = { version: 1; iv: string; ciphertext: string };

async function localKey(create: boolean): Promise<CryptoKey> {
  let encoded = localStorage.getItem(ENCRYPTION_KEY);
  if (encoded) {
    try {
      const bytes = base64ToBytes(encoded);
      if (bytes.length !== 32) throw new Error("invalid key");
      return await crypto.subtle.importKey("raw", bytes, "AES-GCM", false, ["encrypt", "decrypt"]);
    } catch {
      if (!create) throw new Error("本地登录配置无法解密，请重新登录");
      encoded = null;
    }
  }
  if (!encoded && create) {
    encoded = bytesToBase64(crypto.getRandomValues(new Uint8Array(32)));
    localStorage.setItem(ENCRYPTION_KEY,encoded);
  }
  if (!encoded) throw new Error("本地登录配置无法解密，请重新登录");
  return crypto.subtle.importKey("raw",base64ToBytes(encoded),"AES-GCM",false,["encrypt","decrypt"]);
}

/** A local key and ciphertext make the same implementation usable in all clients. */
export async function readLoginConfig(): Promise<SyncConfig | null> {
  const encoded = localStorage.getItem(CONFIG_KEY);
  if (!encoded) return null;
  try {
    const stored = JSON.parse(encoded) as EncryptedConfig;
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
  if (!config) { localStorage.removeItem(CONFIG_KEY); return; }
  const key = await localKey(true), iv = crypto.getRandomValues(new Uint8Array(12));
  const plaintext = new TextEncoder().encode(JSON.stringify(config));
  const ciphertext = await crypto.subtle.encrypt({name:"AES-GCM",iv},key,plaintext);
  localStorage.setItem(CONFIG_KEY,JSON.stringify({version:1,iv:bytesToBase64(iv),ciphertext:bytesToBase64(new Uint8Array(ciphertext))} satisfies EncryptedConfig));
}
