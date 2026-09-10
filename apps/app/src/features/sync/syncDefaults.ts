// App 侧同步配置的默认值。仅供本 app 使用，不进入三端共享的
// @cliproam/protocol；Rust 侧的 serde 兜底（apps/app/src-tauri/src/sync/mod.rs）
// 必须与这里的数值保持一致。
export const DEFAULT_SERVER_PROTOCOL = "http";
export const DEFAULT_AUTO_UPLOAD_LIMIT_MB = 50;
export const DEFAULT_AUTO_UPLOAD_LIMIT = DEFAULT_AUTO_UPLOAD_LIMIT_MB * 1024 * 1024;
export const DEFAULT_AUTO_RECEIVE_CLIPBOARD = true;
// 捕获文件/文件夹时按名称跳过的默认过滤模式（与 Rust 侧 default_exclude_patterns 一致）。
export const DEFAULT_EXCLUDE_PATTERNS = ["node_modules"];
// 服务器单文件存储上限的兜底显示值；登录响应的 settings.maxStoredFileMb 才是权威值。
export const DEFAULT_SERVER_MAX_FILE_MB = 200;
// 单次复制文件数上限的兜底值；登录响应的 settings.maxCaptureFileCount 才是权威值。
export const DEFAULT_MAX_CAPTURE_FILE_COUNT = 1000;
