//! macOS 平台实现：arboard 剪贴板 + osascript 合成 Cmd+V。
//! Windows 测试构建会编译本模块以保持类型检查（见 platforms/mod.rs）。

#![cfg_attr(all(test, target_os = "windows"), allow(dead_code, unused_imports))]

use tauri::{AppHandle, Manager};

pub(crate) use super::arboard_clipboard::{
    read_clipboard_files, read_clipboard_image, read_clipboard_text, write_clipboard_files,
    write_clipboard_image, write_clipboard_text,
};
pub(crate) use super::{
    begin_window_drag, consume_pending_shares, create_windows, deliver_paste, on_paste_window_focus,
    on_window_event, prompt_save_destination, register_plugins, requires_paste_window,
    setup_desktop_shell, should_ignore_paste_focus_loss, should_skip_clipboard_poll,
    show_detached_toast, supports_native_file_export, supports_virtual_file_paste,
    set_virtual_file_clipboard,
};

pub(crate) fn manage_platform_state(app: &AppHandle) -> Result<(), String> {
    let clipboard = super::arboard_clipboard::PlatformClipboard::new()?;
    app.manage(clipboard);
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
