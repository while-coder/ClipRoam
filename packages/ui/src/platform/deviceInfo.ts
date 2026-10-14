import type { DeviceInfo } from "./native";

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
