import { shallowRef } from "vue";
import type { LocalClipboardEntry } from "../../types";

// Registered by the desktop app. Web and mobile do not offer desktop windows.
export const stickyDesktop = shallowRef<((entry: LocalClipboardEntry) => Promise<void>)>();
export const showStickyNotes = shallowRef<(() => Promise<void>)>();
