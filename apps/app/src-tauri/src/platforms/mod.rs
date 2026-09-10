//! 平台适配层：共享代码通过这里的门面调用各系统的特有能力，
//! 所有 `cfg` 都被限制在本模块内部。
//!
//! - `desktop/`：Windows/macOS/Linux 共享实现与各桌面系统目录
//! - `mobile/`：Android/iOS 共享实现与各移动系统目录
//!
//! 模块门上的 `test` 分支让 Windows 的 `cargo test` 也会编译 macOS/Linux
//! 实现（arboard 为 dev-dependency），防止这些代码在类型漂移后静默烂掉。

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod desktop;
#[cfg(any(target_os = "android", target_os = "ios"))]
mod mobile;

#[cfg(target_os = "windows")]
pub(crate) use desktop::windows::*;
#[cfg(target_os = "macos")]
pub(crate) use desktop::macos::*;
#[cfg(target_os = "linux")]
pub(crate) use desktop::linux::*;
#[cfg(target_os = "android")]
pub(crate) use mobile::android::*;
#[cfg(target_os = "ios")]
pub(crate) use mobile::ios::*;

/// 单实例插件回调：第二次启动时唤醒已运行实例的主窗口。移动端由系统保证
/// 单实例，没有这个场景。
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub(crate) fn show_main_window(app: &tauri::AppHandle) {
    let _ = desktop::show_main_window(app);
}

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
