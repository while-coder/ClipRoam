export { native } from "./platform/native";
export { isNative } from "./platform/runtime";
export { isMobile, isQuickPaste, isToastWindow } from "./composables/usePlatform";
export { getAccountSession } from "./features/sync/accountSession";
export { focusSearch, focusSearchInput } from "./features/history/useHistorySync";
