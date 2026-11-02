import { native, type FilePickerMode } from "./native";

export type ClipboardData =
  | { kind: "text"; text: string; html?: string; rtf?: string }
  | { kind: "files"; paths: string[] }
  | { kind: "image"; data: string; width: number; height: number; thumbnail: string };
export type ClipboardWrite = { kind: "text"; text: string; html?: string; rtf?: string }
  | { kind: "files"; paths: string[] } | { kind: "image"; bytes: Uint8Array };

/** Clipboard operations are delegated to the selected platform implementation. */
export const clipboardPlatform = {
  get canWriteImage(): boolean { return native.canWriteImage; },
  monitoringAvailable: native.clipboardMonitoring,
  subscribe: (changed: (data: ClipboardData) => void) => native.listen("cliproam://clipboard-changed", changed),
  read: (): Promise<ClipboardData | null> => native.readClipboard(),
  pickFiles: (mode: FilePickerMode, scope: string) => native.pickFiles(mode, scope),
  write: (payload: ClipboardWrite, synthesize: boolean): Promise<void> => native.writeClipboard(payload, synthesize),
};
