//! Linux 平台实现：arboard 剪贴板 + wtype/ydotool/xdotool 合成 Ctrl+V。

use tauri::{AppHandle, Manager};

pub(crate) use super::arboard_clipboard::{
    read_clipboard_files, read_clipboard_image, read_clipboard_text, write_clipboard_files,
    write_clipboard_image, write_clipboard_text,
};
pub(crate) use super::{
    create_windows, deliver_paste, on_paste_window_focus, on_window_event, register_plugins,
    setup_desktop_shell, should_ignore_paste_focus_loss, show_detached_toast,
};

pub(crate) fn manage_platform_state(app: &AppHandle) -> Result<(), String> {
    let clipboard = super::arboard_clipboard::PlatformClipboard::new()?;
    app.manage(clipboard);
    Ok(())
}

pub(crate) fn synthesize_paste() -> Result<(), String> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let attempts: &[(&str, &[&str])] = if wayland {
        &[
            ("wtype", &["-M", "ctrl", "-k", "v", "-m", "ctrl"]),
            ("ydotool", &["key", "29:1", "47:1", "47:0", "29:0"]),
            ("xdotool", &["key", "--clearmodifiers", "ctrl+v"]),
        ]
    } else {
        &[
            ("xdotool", &["key", "--clearmodifiers", "ctrl+v"]),
            ("ydotool", &["key", "29:1", "47:1", "47:0", "29:0"]),
        ]
    };
    let mut errors = Vec::new();
    for (program, arguments) in attempts {
        match super::run_paste_command(program, arguments) {
            Ok(()) => return Ok(()),
            Err(error) => errors.push(error),
        }
    }
    Err(format!(
        "剪贴板已写入，但无法模拟 Ctrl+V；请安装 {}。{}",
        if wayland {
            "wtype 或 ydotool"
        } else {
            "xdotool"
        },
        errors.join("；")
    ))
}

pub(crate) fn open_data_directory(app: &AppHandle) -> Result<(), String> {
    super::open_data_directory_with(app, "xdg-open")
}

pub(crate) use super::capture_paste_target;
pub(crate) use super::files::{ack_share, pending_shares, pick_files, save_dialog};
pub(crate) use super::show_main_window;
pub(crate) use super::{begin_window_drag, should_skip_clipboard_poll};

pub(crate) use crate::platforms::files::{copy_file, create_directory, remove_file};
