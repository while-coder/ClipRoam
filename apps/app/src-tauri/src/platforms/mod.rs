//! 平台适配层：共享代码通过这里的门面调用各系统的特有能力，
//! 所有 `cfg` 都被限制在本模块内部。
//!
//! 模块门上的 `test` 分支让 Windows 的 `cargo test` 也会编译 macOS/Linux
//! 实现（arboard 为 dev-dependency），防止这些代码在类型漂移后静默烂掉。

#[cfg(target_os = "windows")]
mod windows;
#[cfg(any(target_os = "macos", target_os = "linux", test))]
mod macos;
#[cfg(any(target_os = "macos", target_os = "linux", test))]
mod linux;
#[cfg(any(target_os = "macos", target_os = "linux", test))]
mod arboard_clipboard;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod desktop;
#[cfg(any(target_os = "android", target_os = "ios"))]
mod mobile;
#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;

#[cfg(target_os = "windows")]
pub(crate) use windows::*;
#[cfg(target_os = "macos")]
pub(crate) use macos::*;
#[cfg(target_os = "linux")]
pub(crate) use linux::*;
#[cfg(target_os = "android")]
pub(crate) use android::*;
#[cfg(target_os = "ios")]
pub(crate) use ios::*;

/// 桌面平台在后台线程轮询系统剪贴板；移动端不轮询（Android 通过分享
/// 接收导入），这里把差异收敛成统一入口。
pub(crate) fn start_clipboard_monitor(app: tauri::AppHandle) {
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    crate::clipboard::capture::start_clipboard_monitor(app);
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let _ = app;
    }
}
