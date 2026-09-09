//! Android/iOS 共享实现：没有桌面级的剪贴板轮询、自动粘贴、托盘和
//! 系统对话框。剪贴板读写通过 tauri clipboard-manager 插件，文件通过
//! 系统分享接收（Android 的真实导入在 platforms/android 覆盖）。

use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};

use crate::clipboard::capture::{RichText, ShareImportSummary};
use crate::content::ClipboardEntry;

// ---------------------------------------------------------------------------
// 剪贴板读写
// ---------------------------------------------------------------------------

pub(crate) fn read_clipboard_text(app: &AppHandle) -> Option<RichText> {
    app.clipboard().read_text().ok().and_then(|text| {
        (!text.trim().is_empty()).then_some(RichText { text, html: None, rtf: None })
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

// ---------------------------------------------------------------------------
// 粘贴
// ---------------------------------------------------------------------------

pub(crate) fn synthesize_paste() -> Result<(), String> {
    Err("当前平台不支持自动粘贴".to_string())
}

/// 移动端操作系统不允许普通应用向其他应用注入粘贴。保持 ClipRoam
/// 可见，把激活当作 Copy 处理。
pub(crate) fn deliver_paste(window: &tauri::WebviewWindow, synthesize: bool) -> Result<(), String> {
    let _ = (window, synthesize);
    Ok(())
}

pub(crate) fn requires_paste_window() -> bool {
    false
}

/// 移动端不轮询系统剪贴板（Android 通过分享接收导入）。
pub(crate) fn should_skip_clipboard_poll(_sequence: &mut u32) -> bool {
    false
}

pub(crate) fn supports_virtual_file_paste(_entry: &ClipboardEntry) -> bool {
    false
}

pub(crate) fn set_virtual_file_clipboard(
    _app: &AppHandle,
    _window_label: &str,
    _entry: ClipboardEntry,
) -> Result<(), String> {
    Err("当前平台不支持虚拟文件粘贴".to_string())
}

pub(crate) fn begin_window_drag(_window: &tauri::WebviewWindow) -> Result<(), String> {
    Ok(())
}

// ---------------------------------------------------------------------------
// 应用外壳
// ---------------------------------------------------------------------------

pub(crate) fn on_window_event(_window: &tauri::Window, _event: &tauri::WindowEvent) {}

pub(crate) fn on_paste_window_focus(_app: AppHandle, _focused: bool) {}

pub(crate) fn should_ignore_paste_focus_loss(_app: &AppHandle) -> bool {
    false
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

/// 移动端的主窗口由系统在建应用时创建，不需要（也不能）重建。
pub(crate) fn create_windows(_app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

pub(crate) fn setup_desktop_shell(_app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

pub(crate) fn manage_platform_state(_app: &AppHandle) -> Result<(), String> {
    Ok(())
}

// ---------------------------------------------------------------------------
// 另存与分享导入
// ---------------------------------------------------------------------------

pub(crate) fn supports_native_file_export() -> bool {
    false
}

pub(crate) fn prompt_save_destination(_single_file: bool, _file_name: &str) -> Option<PathBuf> {
    None
}

pub(crate) fn consume_pending_shares(_app: &AppHandle) -> Result<ShareImportSummary, String> {
    Ok(ShareImportSummary::default())
}
