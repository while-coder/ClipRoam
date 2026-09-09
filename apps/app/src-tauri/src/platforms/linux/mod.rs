//! Linux 平台实现：arboard 剪贴板 + wtype/ydotool/xdotool 合成 Ctrl+V。
//! Windows 测试构建会编译本模块以保持类型检查（见 platforms/mod.rs）。

#![cfg_attr(all(test, target_os = "windows"), allow(dead_code, unused_imports))]

use tauri::{AppHandle, Manager};

pub(crate) use super::arboard_clipboard::{
    read_clipboard_files, read_clipboard_image, read_clipboard_text, write_clipboard_files,
    write_clipboard_image, write_clipboard_text,
};
pub(crate) use super::desktop::{
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
        match super::desktop::run_paste_command(program, arguments) {
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
    super::desktop::open_data_directory_with(app, "xdg-open")
}
