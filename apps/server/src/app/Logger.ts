import { mkdirSync } from "node:fs";
import { join } from "node:path";
import log4js, { type Logger } from "log4js";
import { dataDirectory } from "../storage/DataPaths.js";

export const logsDirectory = join(dataDirectory, "logs");
mkdirSync(logsDirectory, { recursive: true });

log4js.configure({
  appenders: {
    console: {
      type: "console",
      layout: { type: "pattern", pattern: "[%d{yyyy-MM-dd hh:mm:ss.SSS}] [%p] %c - %m" },
    },
    file: {
      type: "dateFile",
      encoding: "utf-8",
      filename: join(logsDirectory, "log"),
      alwaysIncludePattern: true,
      pattern: "yyyy-MM-dd.log",
      numBackups: 7,
      layout: { type: "pattern", pattern: "[%d{yyyy-MM-dd hh:mm:ss}] [%p] %c - %m" },
    },
  },
  categories: {
    default: {
      enableCallStack: true,
      appenders: ["console", "file"],
      level: process.env.CLIPROAM_LOG_LEVEL ?? process.env.LOG_LEVEL ?? (process.env.NODE_ENV === "production" ? "INFO" : "ALL"),
    },
  },
});

export function getLogger(name: string): Logger {
  return log4js.getLogger(name);
}

export function shutdownLogger(): Promise<void> {
  return new Promise((resolve, reject) => log4js.shutdown((error) => (error ? reject(error) : resolve())));
}
