//! 应用外壳命令层：平台能力上报、数据目录、窗口拖动/隐藏，以及
//! 主窗隐藏时的 toast 通知。各系统的具体实现见 `platforms/`。

use serde::Serialize;
use std::fs;
use tauri::{AppHandle, Emitter, Manager};

use crate::platforms;
use crate::utils::write_file_atomic;

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

/// 记录当前前台应用，供 macOS 合成粘贴后恢复焦点；其余平台无需处理。
#[cfg(target_os = "macos")]
#[tauri::command]
pub(crate) fn capture_paste_target(app: AppHandle) -> Result<(), String> {
    platforms::capture_paste_target(&app)
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub(crate) fn capture_paste_target(_app: AppHandle) -> Result<(), String> {
    Ok(())
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
    pub(crate) message: String,
    pub(crate) tone: String,
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

// ---------------------------------------------------------------------------
// 设备身份
// ---------------------------------------------------------------------------

/// 机器级设备身份（id + 用户设置的别名），存 `app_data_dir/device.json`，
/// 所有历史档案共用一份。
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceIdentity {
    device_id: String,
    /// 用户设置的显示别名；`None` 表示未设置，展示时回退到系统机器名。
    #[serde(default)]
    device_alias: Option<String>,
}

/// 当前设备身份：读 `device.json` 缓存，缺失则生成 id 并写回。前端还会再
/// 缓存一层，调用频率很低，无需常驻内存。
#[tauri::command]
pub(crate) fn get_device_identity(
    state: tauri::State<'_, crate::AppState>,
) -> Result<DeviceIdentityDto, String> {
    let identity = read_or_create_identity(&state)?;
    Ok(DeviceIdentityDto {
        device_id: identity.device_id,
        device_alias: identity.device_alias,
    })
}

/// 保存设备别名：空串表示清除（回退系统机器名）。写入后由前端在下次
/// 连接同步时随设备信息上报服务器。
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn save_device_alias(
    state: tauri::State<'_, crate::AppState>,
    alias: String,
) -> Result<(), String> {
    let trimmed = alias.trim();
    if trimmed.chars().count() > 80 {
        return Err("设备别名不能超过 80 个字符".to_string());
    }
    let mut identity = read_or_create_identity(&state)?;
    identity.device_alias = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
    let json = serde_json::to_vec_pretty(&identity).map_err(|error| error.to_string())?;
    write_file_atomic(&state.device_config_path, &json)
}

fn read_or_create_identity(
    state: &tauri::State<'_, crate::AppState>,
) -> Result<DeviceIdentity, String> {
    let identity = fs::read_to_string(&state.device_config_path)
        .ok()
        .and_then(|text| serde_json::from_str::<DeviceIdentity>(&text).ok())
        .unwrap_or_else(|| DeviceIdentity {
            device_id: uuid::Uuid::new_v4().to_string(),
            device_alias: None,
        });
    // 原子写回：失败不致命，内存值照常返回，下次调用重试写盘。
    if let Ok(json) = serde_json::to_vec_pretty(&identity) {
        let _ = write_file_atomic(&state.device_config_path, &json);
    }
    Ok(identity)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceIdentityDto {
    device_id: String,
    device_alias: Option<String>,
}

/// 设备展示信息（机器名、CPU、OS 类型与版本、App 版本），除 App 版本外每次
/// 现取、不缓存：机器改名后下次读取立即生效。OS 类型为
/// `windows`/`macos`/`linux`/`android`/`ios`。
#[tauri::command]
pub(crate) fn get_device_info(app: AppHandle) -> Result<DeviceInfo, String> {
    Ok(DeviceInfo {
        device_name: std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "This device".to_string()),
        cpu: std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "未知".to_string()),
        os_type: tauri_plugin_os::platform().to_string(),
        os_version: tauri_plugin_os::version().to_string(),
        app_version: app.package_info().version.to_string(),
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceInfo {
    device_name: String,
    cpu: String,
    os_type: String,
    os_version: String,
    app_version: String,
}
