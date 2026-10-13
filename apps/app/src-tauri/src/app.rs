//! 应用外壳命令层：平台能力上报、数据目录、窗口拖动/隐藏，以及
//! 主窗隐藏时的 toast 通知。各系统的具体实现见 `platforms/`。

use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

use crate::platform;

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
pub(crate) fn native_get_platform_capabilities() -> PlatformCapabilities {
    let mobile = cfg!(any(target_os = "android", target_os = "ios"));
    PlatformCapabilities {
        mobile,
        clipboard_monitoring: true,
        global_shortcut: !mobile,
        automatic_paste: !mobile,
        file_clipboard: !mobile,
        image_clipboard: !mobile,
        native_file_export: !cfg!(target_os = "ios"),
        open_data_directory: !mobile,
        share_receiver: cfg!(target_os = "android"),
    }
}

#[tauri::command]
pub(crate) fn native_open_app_data_dir(app: AppHandle) -> Result<(), String> {
    platform::open_data_directory(&app)
}

#[tauri::command]
pub(crate) fn native_start_window_drag(window: tauri::WebviewWindow) -> Result<(), String> {
    platform::begin_window_drag(&window)
}

/// The revision invalidates delayed blur checks from an earlier window mode.
#[derive(Default)]
pub(crate) struct QuickPasteMode(Mutex<(bool, u64)>);

impl QuickPasteMode {
    pub(crate) fn snapshot(&self) -> (bool, u64) {
        self.0.lock().map(|mode| *mode).unwrap_or_default()
    }
}

pub(crate) fn quick_paste_enabled(app: &AppHandle) -> bool {
    app.try_state::<QuickPasteMode>()
        .map(|mode| mode.snapshot().0)
        .unwrap_or(false)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn native_set_quick_paste_mode(
    window: tauri::WebviewWindow,
    mode: tauri::State<'_, QuickPasteMode>,
    enabled: bool,
) -> Result<bool, String> {
    if window.label() != "main" {
        return Err("仅主窗口可切换快速粘贴模式".to_string());
    }
    let mut mode = mode.0.lock().map_err(|error| error.to_string())?;
    let previous = mode.0;
    mode.0 = enabled;
    mode.1 = mode.1.wrapping_add(1);
    drop(mode);
    if enabled {
        // Focus may have changed while TS was configuring the window.
        platform::on_paste_window_focus(
            window.app_handle().clone(),
            window.is_focused().unwrap_or(false),
        );
    }
    Ok(previous)
}

/// 记录当前前台应用，供 macOS 合成粘贴后恢复焦点；其余平台无需处理。
#[tauri::command]
pub(crate) fn native_capture_paste_target(app: AppHandle) -> Result<(), String> {
    platform::capture_paste_target(&app)
}

/// 注册不绑定 JS Channel 的快捷键，原生插件处理器负责唤起事件。
#[tauri::command]
pub(crate) async fn native_register_quick_paste_shortcut(
    app: AppHandle,
    shortcut: String,
) -> Result<(), String> {
    #[cfg(desktop)]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        app.global_shortcut()
            .register(shortcut.as_str())
            .map_err(|error| error.to_string())?;
        log::debug!("[quick-paste] native shortcut registered");
        Ok(())
    }
    #[cfg(mobile)]
    {
        let _ = (app, shortcut);
        Err("移动端不支持全局快捷键".to_string())
    }
}

/// 前端配置好快速粘贴布局后再显示，避免主界面闪现。
#[tauri::command]
pub(crate) async fn native_focus_quick_paste_window(
    window: tauri::WebviewWindow,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("仅主窗口可显示快速粘贴界面".to_string());
    }
    // macOS 的应用隐藏状态独立于窗口可见状态；先恢复应用再激活窗口。
    #[cfg(target_os = "macos")]
    {
        let target = window.clone();
        tauri::async_runtime::spawn_blocking(move || {
            platform::position_quick_paste_window(&target)
        })
        .await
        .map_err(|error| error.to_string())??;
        window
            .app_handle()
            .show()
            .map_err(|error| error.to_string())?;
    }
    window.show().map_err(|error| error.to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
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
pub(crate) fn native_show_toast(
    app: AppHandle,
    message: String,
    tone: String,
) -> Result<(), String> {
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

    platform::show_detached_toast(&app, payload)
}

#[tauri::command]
pub(crate) fn native_hide_toast(app: AppHandle) -> Result<(), String> {
    let Some(window) = app.get_webview_window("toast") else {
        return Ok(());
    };
    window.hide().map_err(|error| error.to_string())
}

/// 设备展示信息（机器名、CPU、OS 类型与版本、App 版本），除 App 版本外每次
/// 现取、不缓存：机器改名后下次读取立即生效。OS 类型为
/// `windows`/`macos`/`linux`/`android`/`ios`。
/// 设备名：Windows 读 COMPUTERNAME，Linux/macOS 读 HOSTNAME，缺失或为空时
/// 回退系统 hostname，仍为空则用占位名。服务器要求设备名非空，空名会导致
/// WebSocket 认证被拒、登录失败（Android 上两个环境变量都不存在）。
fn read_device_name() -> String {
    let from_env = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let name = if from_env.trim().is_empty() {
        tauri_plugin_os::hostname()
    } else {
        from_env
    };
    if name.trim().is_empty() {
        "未知设备".to_string()
    } else {
        name
    }
}

#[tauri::command]
pub(crate) fn native_get_device_info(app: AppHandle) -> Result<DeviceInfo, String> {
    Ok(DeviceInfo {
        device_name: read_device_name(),
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
