pub(crate) mod toast;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};

use crate::AppState;

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
const TRAY_SHOW_MAIN: &str = "show-main";
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
const TRAY_QUIT: &str = "quit";

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
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let _ = app;
        return Err("移动端应用数据由系统沙箱管理，不能直接打开数据目录".to_string());
    }

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    {
        use std::{fs, process::Command};

        let app_data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
        fs::create_dir_all(&app_data_dir).map_err(|error| error.to_string())?;

        #[cfg(target_os = "windows")]
        Command::new("explorer.exe")
            .arg(&app_data_dir)
            .spawn()
            .map_err(|error| error.to_string())?;
        #[cfg(target_os = "macos")]
        Command::new("open")
            .arg(&app_data_dir)
            .spawn()
            .map_err(|error| error.to_string())?;
        #[cfg(target_os = "linux")]
        Command::new("xdg-open")
            .arg(&app_data_dir)
            .spawn()
            .map_err(|error| error.to_string())?;

        Ok(())
    }
}

#[tauri::command]
pub(crate) fn start_window_drag(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    if window.label() == "paste" {
        let mut guard = state
            .paste_drag_focus_guard
            .lock()
            .map_err(|error| error.to_string())?;
        *guard = Some(Instant::now() + Duration::from_secs(5));
    }
    #[cfg(not(target_os = "windows"))]
    let _ = &state;

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    if let Err(error) = window.start_dragging() {
        #[cfg(target_os = "windows")]
        if let Ok(mut guard) = state.paste_drag_focus_guard.lock() {
            *guard = None;
        }
        return Err(error.to_string());
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let _ = &window;
    Ok(())
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub(crate) fn show_main_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub(crate) fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::{
        menu::{Menu, MenuItem},
        tray::{TrayIconBuilder, TrayIconEvent},
    };

    let show_main = MenuItem::with_id(app, TRAY_SHOW_MAIN, "显示主界面", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, TRAY_QUIT, "退出 ClipRoam", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_main, &quit])?;
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or("application icon is unavailable")?;

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("ClipRoam")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            TRAY_SHOW_MAIN => {
                let _ = show_main_window(app);
            }
            TRAY_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                let _ = show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
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
