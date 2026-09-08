pub(crate) mod toast;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use tauri::PhysicalPosition;
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
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub(crate) fn open_paste(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("paste")
        .ok_or_else(|| "paste window is unavailable".to_string())?;
    position_history_window(&window)?;
    window.show().map_err(|error| error.to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    window
        .emit("cliproam://focus-search", ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[cfg(any(target_os = "android", target_os = "ios"))]
pub(crate) fn open_paste(_app: AppHandle) -> Result<(), String> {
    Err("移动端不支持全局快速粘贴窗口".to_string())
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

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub(crate) fn position_history_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    let cursor = window.cursor_position().map_err(|error| error.to_string())?;
    let Some(monitor) = window
        .monitor_from_point(cursor.x, cursor.y)
        .map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    let window_size = window.outer_size().map_err(|error| error.to_string())?;
    let work_area = monitor.work_area();
    let position = calculate_history_position(
        cursor.x.round() as i32,
        cursor.y.round() as i32,
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
fn calculate_history_position(
    cursor_x: i32,
    cursor_y: i32,
    work_x: i32,
    work_y: i32,
    work_width: u32,
    work_height: u32,
    window_width: u32,
    window_height: u32,
) -> PhysicalPosition<i32> {
    const CURSOR_GAP: i32 = 12;
    const SCREEN_MARGIN: i32 = 8;
    let width = window_width as i32;
    let height = window_height as i32;
    let min_x = work_x + SCREEN_MARGIN;
    let min_y = work_y + SCREEN_MARGIN;
    let max_x = (work_x + work_width as i32 - width - SCREEN_MARGIN).max(min_x);
    let max_y = (work_y + work_height as i32 - height - SCREEN_MARGIN).max(min_y);
    let x = (cursor_x - width / 2).clamp(min_x, max_x);
    let below_cursor = cursor_y + CURSOR_GAP;
    let preferred_y = if below_cursor <= max_y {
        below_cursor
    } else {
        cursor_y - height - CURSOR_GAP
    };

    PhysicalPosition::new(x, preferred_y.clamp(min_y, max_y))
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

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
#[cfg(test)]
mod tests {
    use super::calculate_history_position;

    #[test]
    fn paste_window_position_stays_inside_the_work_area() {
        let position = calculate_history_position(1900, 1050, 0, 0, 1920, 1080, 420, 560);
        assert!(position.x + 420 <= 1920);
        assert!(position.y + 560 <= 1080);
    }
}
