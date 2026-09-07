import { homedir } from "node:os";
import { join, resolve } from "node:path";

export const dataDirectory = resolve(join(homedir(), ".cliproam"));
export const accountsDatabasePath = join(dataDirectory, "accounts.sqlite");
export const filesDatabasePath = join(dataDirectory, "files.sqlite");
export const filesDirectory = join(dataDirectory, "files");
export const usersDirectory = join(dataDirectory, "users");
export const tlsDirectory = join(dataDirectory, "tls");
export const serverSettingsPath = join(dataDirectory, "server-settings.json");

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
