//! macOS 平台实现：arboard 剪贴板 + 原生 AppKit 焦点恢复与 Quartz Cmd+V。
//! Windows 测试构建会编译本模块以保持类型检查（见 platforms/mod.rs）。

#![cfg_attr(all(test, target_os = "windows"), allow(dead_code, unused_imports))]

use std::sync::Mutex;

use tauri::{AppHandle, Manager};

pub(crate) use super::arboard_clipboard::{
    read_clipboard_files, read_clipboard_image, read_clipboard_text, write_clipboard_files,
    write_clipboard_image, write_clipboard_text,
};
pub(crate) use super::{
    create_windows, on_paste_window_focus, on_window_event, register_plugins, setup_desktop_shell,
    should_ignore_paste_focus_loss, show_detached_toast,
};

/// 快捷粘贴窗口弹出前的最前台应用（unix pid）。隐藏粘贴窗口后 macOS 会把
/// 焦点留给自己应用的其他窗口而不是还给之前的应用，合成 Cmd+V 前要靠它
/// 恢复前台。
#[derive(Default)]
pub(crate) struct PasteTarget(Mutex<Option<i32>>);

pub(crate) fn manage_platform_state(app: &AppHandle) -> Result<(), String> {
    let clipboard = super::arboard_clipboard::PlatformClipboard::new()?;
    app.manage(clipboard);
    app.manage(PasteTarget::default());
    #[cfg(target_os = "macos")]
    log::info!(
        "[quick-paste] backend=native executable={:?} accessibility_trusted={}",
        std::env::current_exe().ok(),
        accessibility::is_trusted(),
    );
    Ok(())
}

/// 在用户尝试自动粘贴时请求权限。系统提示是异步的，也可能不重复显示，
/// 所以同时打开辅助功能设置；本次只保留已复制的内容，不继续切焦点或发按键。
#[cfg(target_os = "macos")]
fn ensure_accessibility_access() -> Result<(), String> {
    if accessibility::is_trusted() || accessibility::prompt() {
        return Ok(());
    }
    log::warn!(
        "[quick-paste] accessibility denied for executable={:?}; an enabled entry may belong to an earlier build or require an app restart",
        std::env::current_exe().ok(),
    );
    let settings = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .output();
    let settings_opened = matches!(settings, Ok(ref output) if output.status.success());
    Err(format!(
        "内容已复制。{}请在「辅助功能」中允许 ClipRoam；若已开启仍无效，请完全退出 ClipRoam，删除旧条目后点 + 添加当前应用，再启动重试。",
        if settings_opened { "已打开权限设置。" } else { "设置路径：系统设置 → 隐私与安全性 → 辅助功能。" },
    ))
}

/// 非 macOS 编译（其他桌面平台的测试构建）不触发 macOS 权限流程。
#[cfg(not(target_os = "macos"))]
fn ensure_accessibility_access() -> Result<(), String> {
    Ok(())
}

/// 直接为 ClipRoam 请求辅助功能权限，原生按键投递不依赖 System Events 自动化。
#[cfg(target_os = "macos")]
mod accessibility {
    use std::os::raw::c_void;

    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::{CFString, CFStringRef};

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        static kAXTrustedCheckOptionPrompt: CFStringRef;
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
    }

    /// 当前进程是否已获得辅助功能授权。
    pub(crate) fn is_trusted() -> bool {
        unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) != 0 }
    }

    /// 提示异步显示；返回当前授权状态，不等待用户操作。
    pub(crate) fn prompt() -> bool {
        let prompt_key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
        let options = CFDictionary::from_CFType_pairs(&[(
            prompt_key.as_CFType(),
            CFBoolean::true_value().as_CFType(),
        )]);
        unsafe { AXIsProcessTrustedWithOptions(options.as_CFTypeRef()) != 0 }
    }
}

#[cfg(target_os = "macos")]
fn frontmost_app_pid() -> Option<i32> {
    objc2_app_kit::NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|application| application.processIdentifier())
}

#[cfg(not(target_os = "macos"))]
fn frontmost_app_pid() -> Option<i32> {
    None
}

fn next_paste_target(
    previous: Option<i32>,
    frontmost: Option<i32>,
    own_pid: i32,
    paste_window_visible: bool,
) -> Option<i32> {
    // 再次按快捷键时前台已经是粘贴窗口，保留最初的外部目标。
    if paste_window_visible && frontmost == Some(own_pid) {
        previous.or(frontmost)
    } else {
        frontmost
    }
}

/// 在快捷粘贴窗口显示前调用：此刻前台仍是用户正在操作的应用。
pub(crate) fn capture_paste_target(app: &AppHandle) -> Result<(), String> {
    let paste_window_visible = app
        .get_webview_window("main")
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false)
        && crate::app::quick_paste_enabled(app);
    if let Some(state) = app.try_state::<PasteTarget>() {
        if let Ok(mut guard) = state.0.lock() {
            *guard = next_paste_target(
                *guard,
                frontmost_app_pid(),
                std::process::id() as i32,
                paste_window_visible,
            );
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn reactivate_paste_target(app: &AppHandle) -> Result<(), String> {
    let Some(pid) = app
        .try_state::<PasteTarget>()
        .and_then(|state| state.0.lock().ok().and_then(|guard| *guard))
    else {
        return Err("已复制到剪贴板，但未能识别目标应用，请在目标应用中按 Command+V。".to_string());
    };
    // 目标是自己（粘贴窗口开着时再次按快捷键）就没必要切回来。
    if pid == std::process::id() as i32 {
        return Ok(());
    }
    let target = objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .ok_or_else(|| "目标应用已退出，内容已复制，请手动粘贴。".to_string())?;
    #[allow(deprecated)]
    let activated = target.activateWithOptions(
        objc2_app_kit::NSApplicationActivationOptions::ActivateIgnoringOtherApps,
    );
    if activated {
        // 激活是异步的，确认焦点已交给目标后才能发送按键。
        for _ in 0..20 {
            if frontmost_app_pid() == Some(pid) {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }
    Err("无法激活目标应用，内容已复制，请手动粘贴。".to_string())
}

#[cfg(not(target_os = "macos"))]
fn reactivate_paste_target(_app: &AppHandle) -> Result<(), String> {
    Ok(())
}

/// 弹出粘贴窗口前的前台是否就是 ClipRoam 自己（如在主窗口前台按了快捷键）。
/// 此时没有可粘贴的外部目标。
fn paste_target_is_self(app: &AppHandle) -> bool {
    let own_pid = std::process::id() as i32;
    app.try_state::<PasteTarget>()
        .and_then(|state| state.0.lock().ok().map(|guard| *guard == Some(own_pid)))
        .unwrap_or(false)
}

pub(crate) fn deliver_paste(window: &tauri::WebviewWindow, synthesize: bool) -> Result<(), String> {
    // [paste-debug] 诊断埋点，定位后移除
    tauri_plugin_log::log::info!(
        "[paste-debug] deliver_paste enter synthesize={synthesize} self_target={}",
        paste_target_is_self(window.app_handle())
    );
    if !synthesize {
        return Ok(());
    }
    // 目标是 ClipRoam 自己时没有可自动粘贴的外部输入处；合成 Cmd+V 只会把
    // 内容送进主窗口搜索框并把主界面带到前台。此时仅复制到剪贴板，收起
    // 快速粘贴界面，用户可在真正的目标应用里手动 Cmd+V。
    if paste_target_is_self(window.app_handle()) {
        window.hide().map_err(|error| error.to_string())?;
        return Ok(());
    }
    ensure_accessibility_access()?;
    // 先把前台还给弹出粘贴窗口之前的应用，Cmd+V 才会落到用户原本输入的
    // 地方。先恢复目标应用焦点，再隐藏当前快速粘贴界面。
    reactivate_paste_target(window.app_handle())?;
    std::thread::sleep(std::time::Duration::from_millis(120));
    window.hide().map_err(|error| error.to_string())?;
    // [paste-debug] 诊断埋点，定位后移除
    tauri_plugin_log::log::info!("[paste-debug] target reactivated, paste hidden, synthesizing");
    if let Err(error) = synthesize_paste() {
        // The clipboard content is still valid, but the user needs to see why
        // automatic delivery failed (for example missing macOS Accessibility
        // permission).
        // [paste-debug] 诊断埋点，定位后移除
        tauri_plugin_log::log::info!("[paste-debug] synthesize failed: {error}");
        let _ = window.show();
        return Err(error);
    }
    // [paste-debug] 诊断埋点，定位后移除
    tauri_plugin_log::log::info!("[paste-debug] synthesize ok");
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn synthesize_paste() -> Result<(), String> {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    ensure_accessibility_access()?;
    // Private source 避免继承打开粘贴窗口时仍按住的 Option/Control/Shift。
    let source = CGEventSource::new(CGEventSourceStateID::Private)
        .map_err(|_| "无法创建粘贴按键源".to_string())?;
    const V_KEY: u16 = 9;
    let key_down = CGEvent::new_keyboard_event(source.clone(), V_KEY, true)
        .map_err(|_| "无法创建粘贴按键".to_string())?;
    let key_up = CGEvent::new_keyboard_event(source, V_KEY, false)
        .map_err(|_| "无法创建粘贴按键".to_string())?;
    key_down.set_flags(CGEventFlags::CGEventFlagCommand);
    key_up.set_flags(CGEventFlags::CGEventFlagCommand);
    key_down.post(CGEventTapLocation::HID);
    key_up.post(CGEventTapLocation::HID);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn synthesize_paste() -> Result<(), String> {
    Err("macOS 原生粘贴仅在 macOS 上可用".to_string())
}

pub(crate) fn open_data_directory(app: &AppHandle) -> Result<(), String> {
    super::open_data_directory_with(app, "open")
}

#[cfg(test)]
mod tests {
    use super::next_paste_target;

    #[test]
    fn reopening_paste_window_preserves_external_target() {
        assert_eq!(next_paste_target(Some(42), Some(7), 7, true), Some(42));
    }

    #[test]
    fn new_external_target_replaces_previous_self_target() {
        assert_eq!(next_paste_target(Some(7), Some(42), 7, false), Some(42));
    }

    #[test]
    fn opening_from_main_window_does_not_reuse_previous_external_target() {
        assert_eq!(next_paste_target(Some(42), Some(7), 7, false), Some(7));
    }

    #[test]
    fn unidentified_frontmost_app_does_not_reuse_stale_target() {
        assert_eq!(next_paste_target(Some(42), None, 7, false), None);
    }
}

pub(crate) use super::files::{ack_share, pending_shares, pick_files, save_dialog};
pub(crate) use super::show_main_window;
pub(crate) use super::{begin_window_drag, should_skip_clipboard_poll};

pub(crate) use crate::platforms::files::{copy_file, create_directory, remove_file};
