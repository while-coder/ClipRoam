//! 应用外壳命令层：平台能力上报、数据目录、窗口拖动/隐藏，以及
//! 主窗隐藏时的 toast 通知。各系统的具体实现见 `platforms/`。

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::platforms;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlatformCapabilities {
    mobile: bool,
    clipboard_monitoring: bool,
    global_shortcut: bool,
    automatic_paste: bool,
    file_clipboard: bool,
    image_clipboard: bool,
    native_file_export: bool,
    open_data_directory: bool,
    share_receiver: bool,
}

#[tauri::command]
pub(crate) fn get_platform_capabilities() -> PlatformCapabilities {
    let mobile = cfg!(any(target_os = "android", target_os = "ios"));
    PlatformCapabilities {
        mobile,
        clipboard_monitoring: !mobile,
        global_shortcut: !mobile,
        automatic_paste: !mobile,
        file_clipboard: !mobile,
        image_clipboard: !mobile,
        native_file_export: !mobile,
        open_data_directory: !mobile,
        share_receiver: cfg!(target_os = "android"),
    }
}

#[tauri::command]
pub(crate) fn open_app_data_dir(app: AppHandle) -> Result<(), String> {
    platforms::open_data_directory(&app)
}

#[tauri::command]
pub(crate) fn start_window_drag(window: tauri::WebviewWindow) -> Result<(), String> {
    platforms::begin_window_drag(&window)
}

#[tauri::command]
pub(crate) fn hide_paste(app: AppHandle) -> Result<(), String> {
    app.get_webview_window("paste")
        .ok_or_else(|| "paste window is unavailable".to_string())?
        .hide()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn hide_main(app: AppHandle) -> Result<(), String> {
    app.get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?
        .hide()
        .map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// Toast 通知窗口
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToastPayload {
    message: String,
    tone: String,
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn show_toast(app: AppHandle, message: String, tone: String) -> Result<(), String> {
    let message = message.trim();
    if message.is_empty() {
        return Ok(());
    }
    let payload = ToastPayload {
        message: message.to_string(),
        tone: match tone.as_str() {
            "success" | "error" | "info" => tone,
            _ => "info".to_string(),
        },
    };
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?;
    let main_is_visible =
        main.is_visible().unwrap_or(false) && !main.is_minimized().unwrap_or(false);
    if main_is_visible {
        return main
            .emit("cliproam://toast", payload)
            .map_err(|error| error.to_string());
    }

    platforms::show_detached_toast(&app, payload)
}

#[tauri::command]
pub(crate) fn hide_toast(app: AppHandle) -> Result<(), String> {
    let Some(window) = app.get_webview_window("toast") else {
        return Ok(());
    };
    window.hide().map_err(|error| error.to_string())
}
