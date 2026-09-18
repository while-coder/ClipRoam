import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { DEFAULT_SERVER_ADDRESS } from "../../utils/constants";
import { DEFAULT_SERVER_PROTOCOL, defaultAccountPreferences } from "./syncDefaults";
import type { AccountPreferences, SyncConfig } from "../../types";

/** 登录态完整才允许从登录页返回主界面；token 过期重登时保留返回入口。 */
export const hasSavedSyncConfig = ref(false);
export const currentUsername = ref("");

let activeSyncConfig: SyncConfig | undefined;
/** 活动档案的偏好；跟随档案切换与保存更新（Rust 侧持久化到档案目录）。 */
let activePreferences: AccountPreferences = defaultAccountPreferences();

// 引用恒等是 stale check 的依据（activateRemoteClipboard），访问器不做任何拷贝。
export function getActiveConfig(): SyncConfig | undefined {
  return activeSyncConfig;
}

export function setActiveConfig(config: SyncConfig | undefined): void {
  activeSyncConfig = config;
}

export function getActivePreferences(): AccountPreferences {
  return activePreferences;
}

export function setActivePreferences(preferences: AccountPreferences): void {
  activePreferences = preferences;
}

/** 与 Rust 侧 history_key_for_config 对齐的档案键，仅用于判断是否重登同一账号。 */
export function archiveKeyFor(config: SyncConfig | undefined): string {
  const serverAddress = config?.serverAddress.trim().toLowerCase() ?? "";
  const username = config?.username.trim().toLowerCase() ?? "";
  if (!username) return "";
  return `account:${serverAddress}:${username}`;
}

export async function loadSyncConfig(): Promise<SyncConfig | null> {
  const raw = await invoke<unknown>("get_sync_config");
  if (!raw || typeof raw !== "object") return null;
  const value = raw as Record<string, unknown>;
  return {
    serverAddress: typeof value.serverAddress === "string" && value.serverAddress
      ? value.serverAddress
      : DEFAULT_SERVER_ADDRESS,
    serverProtocol: value.serverProtocol === "https" ? "https" : DEFAULT_SERVER_PROTOCOL,
    username: typeof value.username === "string" ? value.username : "",
    sessionToken: typeof value.sessionToken === "string" ? value.sessionToken : "",
  };
}

export async function persistSyncConfig(config: SyncConfig | null): Promise<void> {
  await invoke("save_sync_config", { config });
  // 配置保存可能切换活动档案（登录/退出账号），偏好跟随档案——
  // 无论是否切换都重读一次，保证前端内存态与活动档案一致。
  activePreferences = await loadAccountPreferences();
}

export async function loadAccountPreferences(): Promise<AccountPreferences> {
  try {
    return await invoke<AccountPreferences>("get_account_preferences");
  } catch {
    return defaultAccountPreferences();
  }
}

export async function persistAccountPreferences(preferences: AccountPreferences): Promise<void> {
  await invoke("save_account_preferences", { preferences });
  activePreferences = preferences;
}
