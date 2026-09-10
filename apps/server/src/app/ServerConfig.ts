import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { ENTRY_PAGE_DEFAULT_LIMIT, ENTRY_PAGE_SIZE_RANGE } from "@cliproam/protocol";
import { serverSettingsPath } from "../DataPaths.js";

const megabyte = 1024 * 1024;
const hour = 60 * 60 * 1_000;

// 每项设置的默认值与合法范围声明在一起，方便对照；管理后台的下拉档位
// 必须落在这些范围内。0 关闭对应功能（文件保存 / 断点续传）。
const SETTING_CONSTRAINTS = {
  maxStoredFileMb: { default: 200, min: 0, max: 2048 },
  resumableUploadTtlHours: { default: 24, min: 0, max: 8760 },
  maxHistoryEntries: { default: 3000, min: 1000, max: 100_000 },
  maxCaptureFileCount: { default: 1000, min: 1, max: 100_000 },
} as const;

// GET /entries/manifest 的每页数量（pageSize 参数）：缺省值与合法范围。
// 范围来自 protocol 的 ENTRY_PAGE_SIZE_RANGE（zod schema 与这里共用）。
export const MANIFEST_PAGE_SIZE = {
  default: ENTRY_PAGE_DEFAULT_LIMIT,
  min: ENTRY_PAGE_SIZE_RANGE.min,
  max: ENTRY_PAGE_SIZE_RANGE.max,
} as const;

// 服务器各处默认配置的统一声明，方便集中查看与调整。
export const SERVER_DEFAULTS = {
  // HTTP/WebSocket 监听端口。
  port: 4810,
  // 全局内容池的垃圾回收周期：回收不再被任何账号条目引用的文件。
  garbageCollectionIntervalMs: 6 * 60 * 60 * 1_000,
  // 账号登录会话的有效期；管理后台会话独立计时。
  accountSessionLifetimeMs: 30 * 24 * 60 * 60 * 1_000,
  adminSessionLifetimeMs: 8 * 60 * 60 * 1_000,
  // 闲置用户库连接的回收阈值与巡检周期（每开一个 SQLite 连接都有成本）。
  userStoreIdleMs: 10 * 60 * 1_000,
  userStoreSweepIntervalMs: 60 * 1_000,
} as const;

export type ServerConfig = {
  port: number;
  maxStoredFileBytes: number;
  resumableUploadTtlMs: number;
  maxHistoryEntries: number;
  maxCaptureFileCount: number;
};

export type TransferSettings = {
  maxStoredFileMb: number;
  resumableUploadTtlHours: number;
  maxHistoryEntries: number;
  maxCaptureFileCount: number;
};

export function loadServerConfig(): ServerConfig {
  const settings = {
    maxStoredFileMb: SETTING_CONSTRAINTS.maxStoredFileMb.default,
    resumableUploadTtlHours: SETTING_CONSTRAINTS.resumableUploadTtlHours.default,
    maxHistoryEntries: SETTING_CONSTRAINTS.maxHistoryEntries.default,
    maxCaptureFileCount: SETTING_CONSTRAINTS.maxCaptureFileCount.default,
    ...readTransferSettings(),
  };
  return {
    port: SERVER_DEFAULTS.port,
    maxStoredFileBytes: settings.maxStoredFileMb * megabyte,
    resumableUploadTtlMs: settings.resumableUploadTtlHours * hour,
    maxHistoryEntries: settings.maxHistoryEntries,
    maxCaptureFileCount: settings.maxCaptureFileCount,
  };
}

export function getTransferSettings(config: ServerConfig): TransferSettings {
  return {
    maxStoredFileMb: config.maxStoredFileBytes / megabyte,
    resumableUploadTtlHours: config.resumableUploadTtlMs / hour,
    maxHistoryEntries: config.maxHistoryEntries,
    maxCaptureFileCount: config.maxCaptureFileCount,
  };
}

export function updateTransferSettings(config: ServerConfig, values: unknown): TransferSettings {
  if (!values || typeof values !== "object") throw new Error("配置必须是对象。");
  const input = values as Record<string, unknown>;
  const settings: TransferSettings = {
    maxStoredFileMb: validateSetting(input.maxStoredFileMb, "服务器文件上限（MB）", SETTING_CONSTRAINTS.maxStoredFileMb),
    resumableUploadTtlHours: validateSetting(input.resumableUploadTtlHours, "断点续传有效期（小时）", SETTING_CONSTRAINTS.resumableUploadTtlHours),
    maxHistoryEntries: validateSetting(input.maxHistoryEntries, "单用户最大历史条数", SETTING_CONSTRAINTS.maxHistoryEntries),
    maxCaptureFileCount: validateSetting(input.maxCaptureFileCount, "单次复制文件数上限", SETTING_CONSTRAINTS.maxCaptureFileCount),
  };
  config.maxStoredFileBytes = settings.maxStoredFileMb * megabyte;
  config.resumableUploadTtlMs = settings.resumableUploadTtlHours * hour;
  config.maxHistoryEntries = settings.maxHistoryEntries;
  config.maxCaptureFileCount = settings.maxCaptureFileCount;
  writeSettings(settings);
  return settings;
}

function readTransferSettings(): Partial<TransferSettings> {
  if (!existsSync(serverSettingsPath)) return {};
  const parsed = JSON.parse(readFileSync(serverSettingsPath, "utf8")) as Record<string, unknown>;
  // A saved value outside the sane range falls back to the default instead of
  // blocking startup.
  return {
    maxStoredFileMb: pickSetting(parsed.maxStoredFileMb, SETTING_CONSTRAINTS.maxStoredFileMb),
    resumableUploadTtlHours: pickSetting(parsed.resumableUploadTtlHours, SETTING_CONSTRAINTS.resumableUploadTtlHours),
    maxHistoryEntries: pickSetting(parsed.maxHistoryEntries, SETTING_CONSTRAINTS.maxHistoryEntries),
    maxCaptureFileCount: pickSetting(parsed.maxCaptureFileCount, SETTING_CONSTRAINTS.maxCaptureFileCount),
  };
}

function writeSettings(settings: TransferSettings): void {
  mkdirSync(dirname(serverSettingsPath), { recursive: true });
  const temporaryPath = `${serverSettingsPath}.${process.pid}.new`;
  writeFileSync(temporaryPath, `${JSON.stringify(settings, null, 2)}\n`, { mode: 0o600 });
  renameSync(temporaryPath, serverSettingsPath);
}

function pickSetting(value: unknown, range: { min: number; max: number }): number | undefined {
  if (typeof value !== "number" || !Number.isInteger(value) || value < range.min || value > range.max) {
    return undefined;
  }
  return value;
}

function validateSetting(value: unknown, name: string, range: { min: number; max: number }): number {
  if (typeof value !== "number" || !Number.isInteger(value) || value < range.min || value > range.max) {
    throw new Error(`${name} 必须是 ${range.min}-${range.max} 之间的整数。`);
  }
  return value;
}
