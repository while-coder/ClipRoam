import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { serverSettingsPath } from "../DataPaths.js";

const megabyte = 1024 * 1024;
const hour = 60 * 60 * 1_000;

export type ServerConfig = {
  port: number;
  maxStoredFileBytes: number;
  resumableUploadTtlMs: number;
};

export type TransferSettings = {
  maxStoredFileMb: number;
  resumableUploadTtlHours: number;
};

export function loadServerConfig(): ServerConfig {
  const defaults: TransferSettings = {
    maxStoredFileMb: 100,
    resumableUploadTtlHours: 24,
  };
  const settings = { ...defaults, ...readTransferSettings() };
  return {
    port: 4810,
    maxStoredFileBytes: settings.maxStoredFileMb * megabyte,
    resumableUploadTtlMs: settings.resumableUploadTtlHours * hour,
  };
}

export function getTransferSettings(config: ServerConfig): TransferSettings {
  return {
    maxStoredFileMb: config.maxStoredFileBytes / megabyte,
    resumableUploadTtlHours: config.resumableUploadTtlMs / hour,
  };
}

export function updateTransferSettings(config: ServerConfig, values: unknown): TransferSettings {
  if (!values || typeof values !== "object") throw new Error("配置必须是对象。");
  const input = values as Record<string, unknown>;
  const settings: TransferSettings = {
    maxStoredFileMb: validateSetting(input.maxStoredFileMb, "服务器文件上限（MB）"),
    resumableUploadTtlHours: validateSetting(input.resumableUploadTtlHours, "断点续传有效期（小时）"),
  };
  config.maxStoredFileBytes = settings.maxStoredFileMb * megabyte;
  config.resumableUploadTtlMs = settings.resumableUploadTtlHours * hour;
  writeSettings(settings);
  return settings;
}

function readTransferSettings(): Partial<TransferSettings> {
  if (!existsSync(serverSettingsPath)) return {};
  const parsed = JSON.parse(readFileSync(serverSettingsPath, "utf8")) as Record<string, unknown>;
  return {
    maxStoredFileMb: validateSetting(parsed.maxStoredFileMb, "已保存的服务器文件上限（MB）"),
    resumableUploadTtlHours: validateSetting(parsed.resumableUploadTtlHours, "已保存的断点续传有效期（小时）"),
  };
}

function writeSettings(settings: TransferSettings): void {
  mkdirSync(dirname(serverSettingsPath), { recursive: true });
  const temporaryPath = `${serverSettingsPath}.${process.pid}.new`;
  writeFileSync(temporaryPath, `${JSON.stringify(settings, null, 2)}\n`, { mode: 0o600 });
  renameSync(temporaryPath, serverSettingsPath);
}

function validateSetting(value: unknown, name: string): number {
  if (typeof value !== "number" || !Number.isInteger(value) || value < 0 || value > 100_000) {
    throw new Error(`${name} 必须是 0-100000 之间的整数。`);
  }
  return value;
}
