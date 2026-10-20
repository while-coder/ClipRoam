import type { DeviceInfo } from "./native";
import type { Device as WireDevice } from "@cliproam/protocol";
import type { Device } from "../types";

/** The server treats deviceInfo as opaque text; decoding belongs to the client. */
export function decodeDevice(device: WireDevice): Device {
  let info: Record<string, unknown> = {};
  try {
    const value: unknown = JSON.parse(device.deviceInfo);
    if (value && typeof value === "object" && !Array.isArray(value)) info = value as Record<string, unknown>;
  } catch {
    // One unreadable payload must not hide the device or break the device list.
  }
  const details: Partial<DeviceInfo> = {};
  for (const key of ["deviceAlias", "deviceName", "deviceModel", "deviceBrand", "deviceMarketName", "appBuildNumber", "cpu", "osType", "osVersion", "appVersion", "architecture", "username", "language", "timeZone", "systemLocale"] as const) {
    if (typeof info[key] === "string") details[key] = info[key];
  }
  if (typeof info.androidApiLevel === "number" && Number.isSafeInteger(info.androidApiLevel) && info.androidApiLevel > 0) {
    details.androidApiLevel = info.androidApiLevel;
  }
  if (typeof info.memoryTotalBytes === "number" && Number.isSafeInteger(info.memoryTotalBytes) && info.memoryTotalBytes > 0) {
    details.memoryTotalBytes = info.memoryTotalBytes;
  }
  return {
    ...details,
    ...device,
    name: details.deviceAlias?.trim() || (typeof info.name === "string" ? info.name.trim() : "") || details.deviceName || device.id,
    // Existing stored payloads used platform before native DeviceInfo was serialized directly.
    platform: details.osType || (typeof info.platform === "string" ? info.platform : "未知"),
  };
}

/** Read live browser/WebView settings; OS-specific information comes from Rust. */
export function getEnvironmentInfo(): Pick<DeviceInfo, "language" | "timeZone"> {
  let timeZone: string | undefined;
  try {
    timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone || undefined;
  } catch {
    // A restricted runtime may not expose a default time zone.
  }
  return { language: navigator.language, ...(timeZone ? { timeZone } : {}) };
}

export function getBrowserInfo(): Pick<DeviceInfo, "osType" | "osVersion"> {
  const brands = (navigator as Navigator & {
    userAgentData?: { brands: readonly { brand: string; version: string }[] };
  }).userAgentData?.brands ?? [];
  // Product brands precede Chromium; brand lists also contain synthetic entries.
  const products = [
    ["Microsoft Edge", "Edge"],
    ["Opera", "Opera"],
    ["Samsung Internet", "Samsung Internet"],
    ["Google Chrome", "Chrome"],
  ] as const;
  for (const [brand, osType] of products) {
    const match = brands.find((entry) => entry.brand === brand && entry.version);
    if (match) return { osType, osVersion: match.version };
  }
  // More specific tokens must precede Chrome/Safari compatibility tokens.
  const patterns: readonly [string, RegExp][] = [
    ["Edge", /(?:Edg|EdgA|EdgiOS|Edge)\/([\d.]+)/],
    ["Opera", /(?:OPR|OPiOS)\/([\d.]+)/],
    ["Samsung Internet", /SamsungBrowser\/([\d.]+)/],
    ["Firefox", /(?:Firefox|FxiOS)\/([\d.]+)/],
    ["Chrome", /(?:Chrome|CriOS)\/([\d.]+)/],
    ["Safari", /Version\/([\d.]+).*Safari\//],
  ];
  for (const [osType, pattern] of patterns) {
    const match = navigator.userAgent.match(pattern);
    if (match) return { osType, osVersion: match[1] };
  }
  const chromium = brands.find((entry) => entry.brand === "Chromium" && entry.version);
  return chromium
    ? { osType: "Chromium", osVersion: chromium.version }
    : { osType: "browser", osVersion: "未知" };
}
