import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { writeFileAtomic } from "../common/atomicWrite.js";
import { dirname } from "node:path";
import { ENTRY_PAGE_DEFAULT_LIMIT, ENTRY_PAGE_SIZE_RANGE } from "@cliproam/protocol";
import { serverSettingsPath } from "../DataPaths.js";

export const MEGABYTE = 1024 * 1024;
export const HOUR = 60 * 60 * 1_000;

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

// 以下均为固定配置（管理后台不可修改），直接以 const 声明。

// HTTP/WebSocket 监听端口。
export const SERVER_PORT = 4810;
// 全局内容池的垃圾回收周期：回收不再被任何账号条目引用的文件。
export const GARBAGE_COLLECTION_INTERVAL_MS = 6 * 60 * 60 * 1_000;
// 账号登录会话的有效期；管理后台会话独立计时。
export const ACCOUNT_SESSION_LIFETIME_MS = 30 * 24 * 60 * 60 * 1_000;
export const ADMIN_SESSION_LIFETIME_MS = 8 * 60 * 60 * 1_000;
// 闲置用户库连接的回收阈值与巡检周期（每开一个 SQLite 连接都有成本）。
export const USER_STORE_IDLE_MS = 10 * 60 * 1_000;
export const USER_STORE_SWEEP_INTERVAL_MS = 60 * 1_000;
// 账号密码的 scrypt 派生密钥长度（字节）。
export const PASSWORD_KEY_LENGTH = 64;
// 登录失败限流：账号登录与管理后台共用同一策略（键分别为 ip:username 与 ip）。
export const LOGIN_MAX_ATTEMPTS = 5;
export const LOGIN_ATTEMPT_WINDOW_MS = 5 * 60 * 1_000;
export const LOGIN_BLOCKED_FOR_MS = 60 * 1_000;
// 管理后台上传 TLS 证书/私钥的单文件大小上限。
export const TLS_MAX_PEM_BYTES = 1_024 * 1_024;

// 运行时可变的传输配置（管理后台可修改）；固定配置见上方各 const。
// 单位与管理后台编辑、落盘、下发 app 的源头值保持一致，不做换算存储。
export type ServerConfig = {
  maxStoredFileMb: number;
  resumableUploadTtlHours: number;
  maxHistoryEntries: number;
  maxCaptureFileCount: number;
};

export function loadServerConfig(): ServerConfig {
  return {
    maxStoredFileMb: SETTING_CONSTRAINTS.maxStoredFileMb.default,
    resumableUploadTtlHours: SETTING_CONSTRAINTS.resumableUploadTtlHours.default,
    maxHistoryEntries: SETTING_CONSTRAINTS.maxHistoryEntries.default,
    maxCaptureFileCount: SETTING_CONSTRAINTS.maxCaptureFileCount.default,
    ...readTransferSettings(),
  };
}

export function updateTransferSettings(config: ServerConfig, values: unknown): ServerConfig {
  if (!values || typeof values !== "object") throw new Error("配置必须是对象。");
  const input = values as Record<string, unknown>;
  config.maxStoredFileMb = validateSetting(input.maxStoredFileMb, "服务器文件上限（MB）", SETTING_CONSTRAINTS.maxStoredFileMb);
  config.resumableUploadTtlHours = validateSetting(input.resumableUploadTtlHours, "断点续传有效期（小时）", SETTING_CONSTRAINTS.resumableUploadTtlHours);
  config.maxHistoryEntries = validateSetting(input.maxHistoryEntries, "单用户最大历史条数", SETTING_CONSTRAINTS.maxHistoryEntries);
  config.maxCaptureFileCount = validateSetting(input.maxCaptureFileCount, "单次复制文件数上限", SETTING_CONSTRAINTS.maxCaptureFileCount);
  writeSettings(config);
  return config;
}

function readTransferSettings(): Partial<ServerConfig> {
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

function writeSettings(settings: ServerConfig): void {
  mkdirSync(dirname(serverSettingsPath), { recursive: true });
  writeFileAtomic(serverSettingsPath, `${JSON.stringify(settings, null, 2)}\n`, 0o600);
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
