//! Android/iOS 共享实现：剪贴板由 clipboard 模块每 2 秒轮询，没有自动粘贴、托盘和
//! 系统对话框。剪贴板读写通过 tauri clipboard-manager 插件，文件通过
//! 系统分享接收（Android 的真实导入在 platforms/mobile/android 覆盖）。

#[cfg(target_os = "android")]
pub(crate) mod android;

use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::clipboard::RichText;

// ---------------------------------------------------------------------------
// 剪贴板读写
// ---------------------------------------------------------------------------

pub(crate) fn read_clipboard_text(app: &AppHandle) -> Option<RichText> {
    app.clipboard().read_text().ok().and_then(|text| {
        (!text.trim().is_empty()).then_some(RichText {
            text,
            html: None,
            rtf: None,
        })
    })
}

pub(crate) fn read_clipboard_files(_app: &AppHandle) -> Option<Vec<PathBuf>> {
    None
}

pub(crate) fn read_clipboard_image(_app: &AppHandle) -> Option<Vec<u8>> {
    None
}

pub(crate) fn write_clipboard_text(app: &AppHandle, rich_text: &RichText) -> Result<(), String> {
    app.clipboard()
        .write_text(&rich_text.text)
        .map_err(|error| error.to_string())
}

pub(crate) fn write_clipboard_files(_app: &AppHandle, _paths: &[String]) -> Result<(), String> {
    Err("当前平台暂不支持文件粘贴".to_string())
}

pub(crate) fn write_clipboard_image(_app: &AppHandle, _image: &[u8]) -> Result<(), String> {
    Err("当前平台暂不支持图片粘贴".to_string())
}

pub(crate) fn open_data_directory(_app: &AppHandle) -> Result<(), String> {
    Err("移动端应用数据由系统沙箱管理，不能直接打开数据目录".to_string())
}

pub(crate) fn show_detached_toast(
    app: &AppHandle,
    payload: crate::app_shell::ToastPayload,
) -> Result<(), String> {
    app.get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?
        .emit("cliproam://toast", payload)
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "ios")]
pub(crate) mod ios;
pub(crate) fn begin_window_drag(_window: &tauri::WebviewWindow) -> Result<(), String> {
    Ok(())
}
pub(crate) fn capture_paste_target(_app: &AppHandle) -> Result<(), String> {
    Ok(())
}
pub(crate) fn deliver_paste(
    _window: &tauri::WebviewWindow,
    _synthesize: bool,
) -> Result<(), String> {
    Ok(())
}
pub(crate) fn on_paste_window_focus(_app: AppHandle, _focused: bool) {}
pub(crate) fn should_skip_clipboard_poll(_sequence: &mut u32) -> bool {
    false
}
pub(crate) fn register_plugins(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    #[cfg(target_os = "android")]
    let builder = builder.plugin(tauri_plugin_cliproam_share_receiver::init());
    builder
}
pub(crate) fn manage_platform_state(_app: &AppHandle) -> Result<(), String> {
    Ok(())
}
pub(crate) fn create_windows(_app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}
pub(crate) fn on_window_event(_window: &tauri::Window, _event: &tauri::WindowEvent) {}
