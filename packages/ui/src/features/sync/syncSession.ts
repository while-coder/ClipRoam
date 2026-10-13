import { ref } from "vue";
import { archiveKeyFor, getAccountSession, replaceAccountSession } from "./accountSession";
export { archiveKeyFor } from "./accountSession";
import { AccountDatabase } from "../../core/database";
import { readLoginConfig, writeLoginConfig } from "../../platform/storage";
import { DEFAULT_SERVER_ADDRESS } from "../../utils/constants";
import { DEFAULT_SERVER_PROTOCOL, defaultAccountPreferences } from "./syncDefaults";
import type { AccountPreferences, SyncConfig } from "../../types";

/** 登录或会话验证通过后，才允许从登录页返回主界面。 */
export const hasSavedSyncConfig = ref(false);
export const currentUsername = ref("");

let activeSyncConfig: SyncConfig | undefined;
/** 活动档案的偏好；跟随账号切换与保存更新（IndexedDB 持久化）。 */
let activePreferences: AccountPreferences = defaultAccountPreferences();

// 引用恒等是 stale check 的依据（activateRemoteClipboard），访问器不做任何拷贝。
export function getActiveConfig(): SyncConfig | undefined {
  return getAccountSession()?.config ?? activeSyncConfig;
}

export function setActiveConfig(config: SyncConfig | undefined): void {
  if (getAccountSession()?.config !== config) replaceAccountSession();
  activeSyncConfig = config;
}

export function getActivePreferences(): AccountPreferences {
  return getAccountSession()?.preferences ?? activePreferences;
}

export function setActivePreferences(preferences: AccountPreferences): void {
  activePreferences = preferences;
  const session = getAccountSession();
  if (session) session.preferences = preferences;
}

let configurationQueue: Promise<unknown> = Promise.resolve();

export async function loadSyncConfig(): Promise<SyncConfig | null> {
  const stored = await readLoginConfig();
  if (!stored) return null;
  return { ...stored, serverAddress:stored.serverAddress || DEFAULT_SERVER_ADDRESS,
    serverProtocol:stored.serverProtocol === "https" ? "https" : DEFAULT_SERVER_PROTOCOL };
}

export function persistSyncConfig(config: SyncConfig | null): Promise<void> {
  const work = configurationQueue.then(() => writeLoginConfig(config));
  configurationQueue = work.catch(() => undefined);
  return work;
}

export async function loadAccountPreferences(): Promise<AccountPreferences> {
  const config = getActiveConfig();
  if (!config?.username) return defaultAccountPreferences();
  const key = archiveKeyFor(config);
  const stored = await new AccountDatabase(key).get<Partial<AccountPreferences>>("meta", "preferences");
  return { ...defaultAccountPreferences(), ...stored };
}

export async function persistAccountPreferences(preferences: AccountPreferences): Promise<void> {
  const config = getActiveConfig();
  if (!config?.username) throw new Error("同步账号未登录");
  const session = getAccountSession();
  await new AccountDatabase(archiveKeyFor(config)).put("meta", "preferences", preferences);
  if (session) { session.preferences = preferences; await session.capture.updateWatch(); }
  activePreferences = preferences;
}
