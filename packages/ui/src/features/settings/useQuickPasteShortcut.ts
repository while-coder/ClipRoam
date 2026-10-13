import { ref } from "vue";
import { native } from "../../platform/native";

// Shared settings bind to the host's controls; registration and storage stay in Tauri.
const controls = native.quickPasteShortcut;
export const quickPasteShortcut = controls?.shortcut ?? ref("");
export const quickPasteShortcutStatus = controls?.status ?? ref({state:"idle" as const,message:""});
export const quickPasteShortcutRefreshing = controls?.refreshing ?? ref(false);
export const displayShortcut = (shortcut: string): string => controls?.display(shortcut) ?? shortcut;
export const resetQuickPasteShortcutDraft = (): void => controls?.resetDraft();
export const saveQuickPasteShortcut = (): Promise<boolean> => controls?.save() ?? Promise.resolve(false);
