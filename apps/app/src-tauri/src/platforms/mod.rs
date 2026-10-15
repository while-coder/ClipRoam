//! 系统具体实现。统一函数入口位于 ../platform.rs，Tauri 命令位于 src 根目录各功能模块。
#[cfg(desktop)]
mod desktop;
#[cfg(mobile)]
mod mobile;
#[cfg(target_os = "linux")]
pub(crate) use desktop::linux as implementation;
#[cfg(target_os = "macos")]
pub(crate) use desktop::macos as implementation;
#[cfg(target_os = "windows")]
pub(crate) use desktop::windows as implementation;
#[cfg(target_os = "android")]
pub(crate) use mobile::android as implementation;
#[cfg(target_os = "ios")]
pub(crate) use mobile::ios as implementation;

mod files;

/// 与 tauri.conf.json 的 identifier 保持一致，供桌面系统启动项使用。
#[cfg(desktop)]
const AUTOSTART_APP_IDENTIFIER: &str = "com.whilecode.cliproam";
