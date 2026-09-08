//! Toast notification window: shown from the tray area whenever the main
//! window is hidden.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use tauri::{PhysicalPosition, Position, Size};

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

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
#[cfg(test)]
mod tests {
    use super::calculate_toast_position;

    #[test]
    fn toast_appears_above_a_bottom_tray() {
        let position =
            calculate_toast_position(1850, 1040, 32, 32, 0, 0, 1920, 1040, 380, 88);
        assert_eq!(position.y, 944);
        assert!(position.x >= 8);
        assert!(position.x + 380 <= 1912);
    }

    #[test]
    fn toast_appears_beside_a_right_tray() {
        let position =
            calculate_toast_position(1920, 850, 40, 32, 0, 0, 1920, 1080, 380, 88);
        assert_eq!(position.x, 1532);
        assert!(position.y >= 8);
        assert!(position.y + 88 <= 1072);
    }
}
