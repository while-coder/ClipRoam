import { homedir } from "node:os";
import { join, resolve } from "node:path";

export const dataDirectory = resolve(process.env.CLIPROAM_DATA_DIRECTORY ?? join(homedir(), ".cliproam"));
export const accountsDatabasePath = resolve(
  process.env.CLIPROAM_ACCOUNTS_DATABASE ?? join(dataDirectory, "accounts.sqlite"),
);
export const usersDirectory = resolve(process.env.CLIPROAM_USERS_DIRECTORY ?? join(dataDirectory, "users"));
export const tlsDirectory = resolve(process.env.CLIPROAM_TLS_DIRECTORY ?? join(dataDirectory, "tls"));
export const serverSettingsPath = resolve(process.env.CLIPROAM_SETTINGS_FILE ?? join(dataDirectory, "server-settings.json"));

function safeUserId(userId: string): string {
  if (!/^[0-9a-f-]{36}$/i.test(userId)) throw new Error("Invalid user ID for storage path");
  return userId;
}

export function userDirectory(userId: string): string {
  return join(usersDirectory, safeUserId(userId));
}

export function userDatabasePath(userId: string): string {
  return join(userDirectory(userId), "data.sqlite");
}

export function userFilesDirectory(userId: string): string {
  return join(userDirectory(userId), "files");
}

export function legacyDatabasePaths(): string[] {
  return [...new Set([
    process.env.CLIPROAM_LEGACY_DATABASE,
    process.env.CLIPROAM_DATABASE,
    join(dataDirectory, "cliproam.sqlite"),
    resolve(process.cwd(), "data", "cliproam.sqlite"),
  ].filter((path): path is string => Boolean(path)).map((path) => resolve(path)))]
    .filter((path) => path !== accountsDatabasePath);
}
