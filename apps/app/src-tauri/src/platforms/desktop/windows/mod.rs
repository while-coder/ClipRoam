//! Windows 平台实现：clipboard-win 剪贴板读写、SendInput 合成 Ctrl+V、
//! 拖拽焦点守卫，以及 COM 虚拟文件剪贴板（见 virtual_files.rs）。

pub(crate) mod virtual_files;

use std::{
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

use super::{open_data_directory_with, spawn_paste_focus_loss_check};
use crate::clipboard::capture::{decode_image_as_bmp, RichText};

pub(crate) use super::{
    consume_pending_shares, create_windows, deliver_paste, on_window_event,
    prompt_save_destination, register_plugins, requires_paste_window, setup_desktop_shell,
    show_detached_toast, supports_native_file_export,
};
pub(crate) use virtual_files::{
    set_clipboard as set_virtual_file_clipboard,
    supports_entry as supports_virtual_file_paste,
};

// ---------------------------------------------------------------------------
// 剪贴板读取
// ---------------------------------------------------------------------------

pub(crate) fn read_clipboard_files(_app: &AppHandle) -> Option<Vec<PathBuf>> {
    clipboard_win::get_clipboard(clipboard_win::formats::FileList).ok()
}

pub(crate) fn read_clipboard_image(_app: &AppHandle) -> Option<Vec<u8>> {
    use clipboard_win::{formats::Bitmap, Clipboard, Getter};

    let _clipboard = Clipboard::new_attempts(10).ok()?;
    let mut image = Vec::new();
    Bitmap.read_clipboard(&mut image).ok()?;
    (!image.is_empty()).then_some(image)
}

pub(crate) fn read_clipboard_text(_app: &AppHandle) -> Option<RichText> {
    use clipboard_win::{
        formats::{Html, RawData, Unicode},
        raw, Clipboard, Getter,
    };

    let _clipboard = Clipboard::new_attempts(10).ok()?;
    let mut text = String::new();
    Unicode.read_clipboard(&mut text).ok()?;
    if text.trim().is_empty() {
        return None;
    }

    let html = Html::new().and_then(|format| {
        let mut value = String::new();
        format
            .read_clipboard(&mut value)
            .ok()
            .filter(|_| !value.is_empty())
            .map(|_| value)
    });
    let rtf = raw::register_format("Rich Text Format").and_then(|format| {
        let mut value = Vec::new();
        RawData(format.get())
            .read_clipboard(&mut value)
            .ok()
            .and_then(|_| String::from_utf8(value).ok())
            .map(|value| value.trim_end_matches('\0').to_string())
            .filter(|value| !value.is_empty())
    });

    Some(RichText { text, html, rtf })
}

// ---------------------------------------------------------------------------
// 剪贴板写入
// ---------------------------------------------------------------------------

pub(crate) fn write_clipboard_text(_app: &AppHandle, rich_text: &RichText) -> Result<(), String> {
    use clipboard_win::{
        formats::{Html, Unicode},
        raw, Clipboard, Setter,
    };

    let _clipboard = Clipboard::new_attempts(10).map_err(|error| error.to_string())?;
    Unicode
        .write_clipboard(&rich_text.text)
        .map_err(|error| error.to_string())?;
    if let Some(html) = &rich_text.html {
        if let Some(format) = Html::new() {
            format
                .write_clipboard(html)
                .map_err(|error| error.to_string())?;
        }
    }
    if let Some(rtf) = &rich_text.rtf {
        let format = raw::register_format("Rich Text Format")
            .ok_or_else(|| "无法注册 RTF 剪贴板格式".to_string())?;
        let mut rtf = rtf.clone().into_bytes();
        rtf.push(0);
        raw::set_without_clear(format.get(), &rtf).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(crate) fn write_clipboard_files(_app: &AppHandle, paths: &[String]) -> Result<(), String> {
    use clipboard_win::{formats::FileList, Clipboard, Setter};

    let _clipboard = Clipboard::new_attempts(10).map_err(|error| error.to_string())?;
    FileList
        .write_clipboard(paths)
        .map_err(|error| error.to_string())
}

pub(crate) fn write_clipboard_image(_app: &AppHandle, image: &[u8]) -> Result<(), String> {
    use clipboard_win::{options::DoClear, raw, Clipboard};

    let bitmap = decode_image_as_bmp(image)?;
    let _clipboard = Clipboard::new_attempts(10).map_err(|error| error.to_string())?;
    // clipboard-win 5.x keeps existing formats when setting a bitmap. Clear
    // them explicitly so a text-only target cannot paste stale Unicode text
    // from the previous clipboard value when it rejects the image format.
    raw::set_bitmap_with(&bitmap, DoClear).map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// 粘贴合成
// ---------------------------------------------------------------------------

pub(crate) fn synthesize_paste() -> Result<(), String> {
    use std::mem::size_of;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL, VK_V,
    };
    fn key(vk: u16, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }
    let inputs = [
        key(VK_CONTROL, 0),
        key(VK_V, 0),
        key(VK_V, KEYEVENTF_KEYUP),
        key(VK_CONTROL, KEYEVENTF_KEYUP),
    ];
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent == inputs.len() as u32 {
        Ok(())
    } else {
        Err(format!("SendInput inserted {sent} of {} events", inputs.len()))
    }
}

// ---------------------------------------------------------------------------
// 轮询节奏：按剪贴板序列号跳过未变化的轮询
// ---------------------------------------------------------------------------

pub(crate) fn should_skip_clipboard_poll(sequence: &mut u32) -> bool {
    let sequence_number = unsafe {
        windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber()
    };
    if sequence_number != 0 && sequence_number == *sequence {
        std::thread::sleep(Duration::from_millis(350));
        return true;
    }
    *sequence = sequence_number;
    false
}

// ---------------------------------------------------------------------------
// 拖拽焦点守卫：快捷粘贴窗口拖动后短暂忽略焦点丢失，避免系统拖拽
// 结束时的焦点抖动把窗口藏起来。
// ---------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct PasteDragFocus(Mutex<Option<Instant>>);

pub(crate) fn on_paste_window_focus(app: AppHandle, focused: bool) {
    if focused {
        if let Some(state) = app.try_state::<PasteDragFocus>() {
            if let Ok(mut guard) = state.0.lock() {
                *guard = None;
            }
        }
        return;
    }
    spawn_paste_focus_loss_check(app);
}

pub(crate) fn should_ignore_paste_focus_loss(app: &AppHandle) -> bool {
    let Some(state) = app.try_state::<PasteDragFocus>() else {
        return false;
    };
    state
        .0
        .lock()
        .map(|mut guard| match *guard {
            Some(deadline) if Instant::now() <= deadline => true,
            Some(_) => {
                *guard = None;
                false
            }
            None => false,
        })
        .unwrap_or(false)
}

pub(crate) fn begin_window_drag(window: &tauri::WebviewWindow) -> Result<(), String> {
    let set_guard = |value: Option<Instant>| {
        if let Some(state) = window.app_handle().try_state::<PasteDragFocus>() {
            if let Ok(mut guard) = state.0.lock() {
                *guard = value;
            }
        }
    };
    if window.label() == "paste" {
        set_guard(Some(Instant::now() + Duration::from_secs(5)));
    }
    if let Err(error) = window.start_dragging() {
        set_guard(None);
        return Err(error.to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 其他
// ---------------------------------------------------------------------------

pub(crate) fn open_data_directory(app: &AppHandle) -> Result<(), String> {
    open_data_directory_with(app, "explorer.exe")
}

pub(crate) fn manage_platform_state(app: &AppHandle) -> Result<(), String> {
    app.manage(PasteDragFocus::default());
    Ok(())
}
