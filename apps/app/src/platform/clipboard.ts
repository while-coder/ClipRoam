import { native } from "./native";

export type ClipboardData =
  | { kind: "text"; text: string; html?: string; rtf?: string }
  | { kind: "files"; paths: string[] }
  | { kind: "image"; data: string; width: number; height: number; thumbnail: string };
export type ClipboardCapture = { id: string; signature: string; data: ClipboardData };
export type ClipboardEvent = { scope: string; capture: ClipboardCapture };
export type SharedContent = { id: string; text?: string; html?: string; images: string[]; paths: string[] };
export type ClipboardWrite = { kind: "text"; text: string; html?: string; rtf?: string }
  | { kind: "files"; paths: string[] } | { kind: "image"; bytes: Uint8Array };

/** Clipboard operations are delegated to the selected platform implementation. */
export const clipboardPlatform = {
  monitoringAvailable: native.clipboardMonitoring,
  subscribe: (changed: (event: ClipboardEvent) => void) => native.listen("cliproam://clipboard-changed", changed),
  watch: (scope: string, enabled: boolean): Promise<void> => native.watchClipboard(scope, enabled),
  shares: (scope: string): Promise<SharedContent[]> => native.pendingShares(scope),
  acknowledgeShare: (id: string): Promise<void> => native.acknowledgeShare(id),
  readText: (): Promise<ClipboardData | null> => native.readClipboard(true),
  pickFiles: (mode: string, scope: string) => native.pickFiles(mode, scope),
  write: (payload: ClipboardWrite, synthesize: boolean): Promise<void> => native.writeClipboard(payload, synthesize),
};
