import type { AccountPreferences } from "../../types";

/** Apply only recognized, usable values; config changes must never reject a session. */
export function applyServerSettings(preferences: AccountPreferences, settings: unknown): AccountPreferences {
  if (!settings || typeof settings !== "object" || Array.isArray(settings)) return preferences;
  const values = settings as Record<string, unknown>;
  const readLimit = (key: string, fallback: number, min: number): number => {
    const value = values[key];
    return typeof value === "number" && Number.isSafeInteger(value) && value >= min ? value : fallback;
  };
  const serverMaxFileMb = readLimit("maxStoredFileMb", preferences.serverMaxFileMb, 0);
  return {
    ...preferences,
    serverMaxFileMb,
    serverMaxImageMb: readLimit("maxStoredImageMb", preferences.serverMaxImageMb, 0),
    serverMaxHistoryEntries: readLimit("maxHistoryEntries", preferences.serverMaxHistoryEntries, 1),
    maxCaptureFileCount: readLimit("maxCaptureFileCount", preferences.maxCaptureFileCount, 1),
    autoUploadLimitMb: Math.min(preferences.autoUploadLimitMb, serverMaxFileMb),
  };
}
