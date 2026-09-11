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
    Ok(())
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
            *guard = frontmost_app_pid();
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

pub(crate) fn deliver_paste(window: &tauri::WebviewWindow, synthesize: bool) -> Result<(), String> {
    if !synthesize {
        return Ok(());
    }
    window.hide().map_err(|error| error.to_string())?;
    // 先把前台还给弹出粘贴窗口之前的应用，Cmd+V 才会落到用户原本输入的
    // 地方；否则主界面可见时会被粘贴进主界面的搜索框。
    reactivate_paste_target(window.app_handle());
    std::thread::sleep(std::time::Duration::from_millis(120));
    if let Err(error) = synthesize_paste() {
        // The clipboard content is still valid, but the user needs to see why
        // automatic delivery failed (for example missing macOS Accessibility
        // permission).
        let _ = window.show();
        return Err(error);
    }
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
        format!("无法模拟 Command+V，请在系统设置中允许 ClipRoam 使用辅助功能：{error}")
    })
}

pub(crate) fn open_data_directory(app: &AppHandle) -> Result<(), String> {
    super::open_data_directory_with(app, "open")
}
