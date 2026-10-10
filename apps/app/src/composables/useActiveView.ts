import { ref } from "vue";

/**
 * 主窗口当前视图（模块级单例，对齐 usePlatform 风格）。后台模块（如
 * 待同步列表按需刷新、下载页轮询兜底）也需要读它。
 */
export const activeView = ref<"history" | "pending-sync" | "uploads" | "downloads">("history");
