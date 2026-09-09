//! 窗口与托盘：平台能力、无边框窗口拖动、托盘菜单，以及主窗隐藏时
//! 从托盘区域弹出的 toast 通知窗口。

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use tauri::{PhysicalPosition, Position, Size};

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

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    {
        let window = app
            .get_webview_window("toast")
            .ok_or_else(|| "toast window is unavailable".to_string())?;
        position_toast_window(&app, &window)?;
        window
            .emit("cliproam://toast", payload)
            .map_err(|error| error.to_string())?;
        window.show().map_err(|error| error.to_string())?;
        return Ok(());
    }

    #[cfg(any(target_os = "android", target_os = "ios"))]
    main.emit("cliproam://toast", payload)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn hide_toast(app: AppHandle) -> Result<(), String> {
    let Some(window) = app.get_webview_window("toast") else {
        return Ok(());
    };
    window.hide().map_err(|error| error.to_string())
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn position_toast_window(app: &AppHandle, window: &tauri::WebviewWindow) -> Result<(), String> {
    let window_size = window.outer_size().map_err(|error| error.to_string())?;
    let scale_factor = window.scale_factor().map_err(|error| error.to_string())?;
    let tray_rect = app
        .tray_by_id("main")
        .ok_or_else(|| "tray icon is unavailable".to_string())?
        .rect()
        .map_err(|error| error.to_string())?;

    let (tray_position, tray_size, monitor) = if let Some(rect) = tray_rect {
        let position = match rect.position {
            Position::Physical(position) => position.cast::<i32>(),
            Position::Logical(position) => position.to_physical::<i32>(scale_factor),
        };
        let size = match rect.size {
            Size::Physical(size) => size.cast::<u32>(),
            Size::Logical(size) => size.to_physical::<u32>(scale_factor),
        };
        let monitor = window
            .monitor_from_point(
                f64::from(position.x) + f64::from(size.width) / 2.0,
                f64::from(position.y) + f64::from(size.height) / 2.0,
            )
            .map_err(|error| error.to_string())?
            .or(window.primary_monitor().map_err(|error| error.to_string())?)
            .ok_or_else(|| "monitor is unavailable".to_string())?;
        (position, size, monitor)
    } else {
        // Linux tray implementations do not expose icon bounds. Anchor the
        // toast to the primary work area's bottom-right corner instead.
        let monitor = window
            .primary_monitor()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "monitor is unavailable".to_string())?;
        let work_area = monitor.work_area();
        (
            PhysicalPosition::new(
                work_area.position.x + work_area.size.width as i32 - 24,
                work_area.position.y + work_area.size.height as i32,
            ),
            tauri::PhysicalSize::new(24, 24),
            monitor,
        )
    };
    let work_area = monitor.work_area();
    let position = calculate_toast_position(
        tray_position.x,
        tray_position.y,
        tray_size.width,
        tray_size.height,
        work_area.position.x,
        work_area.position.y,
        work_area.size.width,
        work_area.size.height,
        window_size.width,
        window_size.height,
    );
    window
        .set_position(position)
        .map_err(|error| error.to_string())
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
#[allow(clippy::too_many_arguments)]
fn calculate_toast_position(
    tray_x: i32,
    tray_y: i32,
    tray_width: u32,
    tray_height: u32,
    work_x: i32,
    work_y: i32,
    work_width: u32,
    work_height: u32,
    window_width: u32,
    window_height: u32,
) -> PhysicalPosition<i32> {
    const GAP: i32 = 8;
    const MARGIN: i32 = 8;
    const EDGE_TOLERANCE: i32 = 4;
    let tray_width = tray_width as i32;
    let tray_height = tray_height as i32;
    let window_width = window_width as i32;
    let window_height = window_height as i32;
    let work_right = work_x + work_width as i32;
    let work_bottom = work_y + work_height as i32;
    let tray_right = tray_x + tray_width;
    let tray_bottom = tray_y + tray_height;
    let centered_x = tray_x + tray_width / 2 - window_width / 2;
    let centered_y = tray_y + tray_height / 2 - window_height / 2;

    let (x, y) = if tray_y >= work_bottom - EDGE_TOLERANCE {
        (centered_x, tray_y - window_height - GAP)
    } else if tray_bottom <= work_y + EDGE_TOLERANCE {
        (centered_x, tray_bottom + GAP)
    } else if tray_x >= work_right - EDGE_TOLERANCE {
        (tray_x - window_width - GAP, centered_y)
    } else if tray_right <= work_x + EDGE_TOLERANCE {
        (tray_right + GAP, centered_y)
    } else {
        (centered_x, tray_y - window_height - GAP)
    };
    let max_x = (work_right - window_width - MARGIN).max(work_x + MARGIN);
    let max_y = (work_bottom - window_height - MARGIN).max(work_y + MARGIN);
    PhysicalPosition::new(
        x.clamp(work_x + MARGIN, max_x),
        y.clamp(work_y + MARGIN, max_y),
    )
}
