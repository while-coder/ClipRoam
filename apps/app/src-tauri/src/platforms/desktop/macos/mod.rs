//! macOS 平台实现：arboard 剪贴板 + osascript 合成 Cmd+V。
//! Windows 测试构建会编译本模块以保持类型检查（见 platforms/mod.rs）。

#![cfg_attr(all(test, target_os = "windows"), allow(dead_code, unused_imports))]

use std::sync::Mutex;

use tauri::{AppHandle, Manager};

pub(crate) use super::arboard_clipboard::{
    read_clipboard_files, read_clipboard_image, read_clipboard_text, write_clipboard_files,
    write_clipboard_image, write_clipboard_text,
};
pub(crate) use super::{
    begin_window_drag, consume_pending_shares, create_windows, on_paste_window_focus,
    on_window_event, prompt_save_destination, register_plugins, requires_paste_window,
    setup_desktop_shell, should_ignore_paste_focus_loss, should_skip_clipboard_poll,
    show_detached_toast, supports_native_file_export, supports_virtual_file_paste,
    set_virtual_file_clipboard,
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
    ensure_accessibility_prompted();
    Ok(())
}

/// 启动时未授权就触发系统弹窗：用户点"打开系统设置"后列表里就有
/// ClipRoam 了；不弹的话列表里永远找不到它，快捷粘贴必然失败。
#[cfg(target_os = "macos")]
fn ensure_accessibility_prompted() {
    if !accessibility::is_trusted() {
        accessibility::prompt();
    }
}

/// 非 macOS 编译（Windows 测试构建等）无辅助功能权限一说，空实现。
#[cfg(not(target_os = "macos"))]
fn ensure_accessibility_prompted() {}

/// 辅助功能（Accessibility）权限检测。合成 Cmd+V 走 osascript，TCC 会把
/// 按键行为追溯到 ClipRoam 本身，未授权时报 1002 且应用不会自动出现在
/// 辅助功能列表里——只有主动调 AXIsProcessTrustedWithOptions 弹窗，系统
/// 才会把 ClipRoam 加进列表。
#[cfg(target_os = "macos")]
mod accessibility {
    use std::os::raw::c_void;

    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;

    extern "C" {
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
    }

    /// 当前进程是否已获得辅助功能授权。
    pub(crate) fn is_trusted() -> bool {
        unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) != 0 }
    }

    /// 触发系统授权弹窗，系统会把 ClipRoam 自动加进辅助功能列表。
    /// 返回调用后是否已授信（用户当场同意才为 true）。
    pub(crate) fn prompt() -> bool {
        let options = CFDictionary::from_CFType_pairs(&[(
            CFString::new("AXTrustedCheckOptionPrompt").as_CFType(),
            CFBoolean::true_value().as_CFType(),
        )]);
        unsafe { AXIsProcessTrustedWithOptions(options.as_CFTypeRef()) != 0 }
    }
}

fn frontmost_app_pid() -> Option<i32> {
    let output = std::process::Command::new("osascript")
        .args([
            "-e",
            "tell application \"System Events\" to unix id of first application process whose frontmost is true",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// 在快捷粘贴窗口显示前调用：此刻前台仍是用户正在操作的应用。
pub(crate) fn capture_paste_target(app: &AppHandle) -> Result<(), String> {
    if let Some(state) = app.try_state::<PasteTarget>() {
        if let Ok(mut guard) = state.0.lock() {
            // 前台是自己的窗口（粘贴窗口开着时再次按快捷键）时保留原目标：
            // 用户真正要粘贴的仍是第一次弹出粘贴窗口前的前台应用。
            let own_pid = std::process::id() as i32;
            if *guard != Some(own_pid) {
                *guard = frontmost_app_pid();
            }
        }
    }
    Ok(())
}

fn reactivate_paste_target(app: &AppHandle) {
    let Some(pid) = app
        .try_state::<PasteTarget>()
        .and_then(|state| state.0.lock().ok().and_then(|guard| *guard))
    else {
        return;
    };
    // 目标是自己（粘贴窗口开着时再次按快捷键）就没必要切回来。
    if pid == std::process::id() as i32 {
        return;
    }
    let _ = std::process::Command::new("osascript")
        .args([
            "-e",
            &format!(
                "tell application \"System Events\" to set frontmost of (first application process whose unix id is {pid}) to true"
            ),
        ])
        .output();
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
    tauri_plugin_log::log::info!("[paste-debug] deliver_paste enter synthesize={synthesize} self_target={}", paste_target_is_self(window.app_handle()));
    if !synthesize {
        return Ok(());
    }
    // 目标是 ClipRoam 自己时没有可自动粘贴的外部输入处；合成 Cmd+V 只会把
    // 内容送进主窗口搜索框并把主界面带到前台。此时仅复制到剪贴板，收起
    // 粘贴窗口与主界面，用户可在真正的目标应用里手动 Cmd+V。
    if paste_target_is_self(window.app_handle()) {
        window.hide().map_err(|error| error.to_string())?;
        if let Some(main) = window.app_handle().get_webview_window("main") {
            let _ = main.hide();
        }
        return Ok(());
    }
    // 先把前台还给弹出粘贴窗口之前的应用，Cmd+V 才会落到用户原本输入的
    // 地方；否则主界面可见时会被粘贴进主界面的搜索框。必须先切前台再隐藏
    // 粘贴窗口：反过来时 macOS 会把焦点落给应用内下一个可见窗口（主界面），
    // 主界面会闪现一下。
    reactivate_paste_target(window.app_handle());
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

pub(crate) fn synthesize_paste() -> Result<(), String> {
    super::run_paste_command(
        "osascript",
        &[
            "-e",
            "tell application \"System Events\" to keystroke \"v\" using command down",
        ],
    )
    .map_err(|error| {
        // osascript 的原始报错（如 1002）对用户没有指导意义；未授权时直接
        // 给出可操作的路径。
        #[cfg(target_os = "macos")]
        if !accessibility::is_trusted() {
            return "快速粘贴需要在 系统设置 → 隐私与安全性 → 辅助功能 中打开 \
                    ClipRoam；列表里没有就点 + 添加\"应用程序\"目录里的 ClipRoam"
                .to_string();
        }
        format!("无法模拟 Command+V，请在系统设置中允许 ClipRoam 使用辅助功能：{error}")
    })
}

pub(crate) fn open_data_directory(app: &AppHandle) -> Result<(), String> {
    super::open_data_directory_with(app, "open")
}
