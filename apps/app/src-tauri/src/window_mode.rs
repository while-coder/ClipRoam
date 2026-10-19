//! Native geometry and focus transitions for the shared main/quick-paste window.
use serde::Deserialize;
#[cfg(desktop)]
use std::sync::Mutex;
use tauri::WebviewWindow;
#[cfg(desktop)]
use tauri::{
    LogicalSize, LogicalUnit, Manager, PhysicalPosition, PhysicalSize, WindowSizeConstraints,
};

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WindowMode {
    Main,
    QuickPaste,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum WindowPosition {
    Cursor,
    Center,
    /// Global physical pixels, matching Tauri's PhysicalPosition.
    Coordinates {
        x: i32,
        y: i32,
    },
}

#[cfg(desktop)]
struct MainGeometry {
    size: PhysicalSize<u32>,
    position: PhysicalPosition<i32>,
    maximized: bool,
    fullscreen: bool,
}

#[cfg(desktop)]
#[derive(Default)]
struct Geometry {
    quick: bool,
    main: Option<MainGeometry>,
    quick_height: Option<f64>,
}

#[cfg(desktop)]
#[derive(Default)]
pub(crate) struct WindowModeState(Mutex<Geometry>);

#[cfg(mobile)]
#[derive(Default)]
pub(crate) struct WindowModeState;

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn native_set_window_mode(
    window: WebviewWindow,
    mode: WindowMode,
    title: Option<String>,
    position: Option<WindowPosition>,
    visible: Option<bool>,
) -> Result<(), String> {
    #[cfg(mobile)]
    {
        let _ = (window, mode, title, position, visible);
        Err("移动端不支持切换窗口模式".into())
    }
    #[cfg(desktop)]
    {
        if window.label() != "main" {
            return Err("仅主窗口可切换窗口模式".into());
        }
        // Window operations and macOS AppKit dispatch must not block the UI thread.
        tauri::async_runtime::spawn_blocking(move || {
            apply_mode(&window, mode, title, position, visible.unwrap_or(true))
        })
        .await
        .map_err(|error| error.to_string())?
    }
}

#[cfg(desktop)]
fn apply_mode(
    window: &WebviewWindow,
    mode: WindowMode,
    title: Option<String>,
    position: Option<WindowPosition>,
    visible: bool,
) -> Result<(), String> {
    let state = window.state::<WindowModeState>();
    let mut geometry = state.0.lock().map_err(|error| error.to_string())?;
    let previous = geometry.quick;
    let quick = mode == WindowMode::QuickPaste;
    crate::app::set_quick_paste_mode(window, false)?;
    let configure = (|| -> tauri::Result<()> {
        if previous != quick || !visible {
            window.hide()?;
        }
        if quick && !previous {
            let maximized = window.is_maximized()?;
            let fullscreen = window.is_fullscreen()?;
            if fullscreen {
                window.set_fullscreen(false)?;
            }
            if maximized {
                window.unmaximize()?;
            }
            // Save before mutating constraints or size, including during failed transitions.
            if geometry.main.is_none() {
                geometry.main = Some(MainGeometry {
                    size: window.inner_size()?,
                    position: window.outer_position()?,
                    maximized,
                    fullscreen,
                });
            }
            window.set_size_constraints(WindowSizeConstraints {
                min_width: Some(LogicalUnit(520.0).into()),
                max_width: Some(LogicalUnit(520.0).into()),
                min_height: Some(LogicalUnit(520.0).into()),
                ..Default::default()
            })?;
            window.set_decorations(false)?;
            window.set_always_on_top(true)?;
            #[cfg(not(target_os = "macos"))]
            window.set_skip_taskbar(true)?;
            window.set_size(LogicalSize::new(
                520.0,
                geometry.quick_height.unwrap_or(640.0),
            ))?;
            geometry.quick = true;
        } else if !quick && (previous || geometry.main.is_some()) {
            if previous {
                geometry.quick_height = Some(
                    window
                        .inner_size()?
                        .to_logical::<f64>(window.scale_factor()?)
                        .height,
                );
            }
            window.set_decorations(true)?;
            window.set_always_on_top(false)?;
            #[cfg(not(target_os = "macos"))]
            window.set_skip_taskbar(false)?;
            window.set_size_constraints(WindowSizeConstraints {
                min_width: Some(LogicalUnit(840.0).into()),
                min_height: Some(LogicalUnit(600.0).into()),
                ..Default::default()
            })?;
            if let Some(main) = &geometry.main {
                window.set_size(main.size)?;
                window.set_position(main.position)?;
                if main.maximized {
                    window.maximize()?;
                }
                if main.fullscreen {
                    window.set_fullscreen(true)?;
                }
            }
            geometry.main = None;
            geometry.quick = false;
        }
        window.set_title(title.as_deref().unwrap_or(if quick {
            crate::branding::quick_paste_title()
        } else {
            crate::branding::app_name()
        }))?;
        Ok(())
    })();
    configure.map_err(|error| error.to_string())?;
    if let Some(position) = position {
        match position {
            WindowPosition::Cursor => position_at_cursor(window)?,
            WindowPosition::Center => window.center().map_err(|error| error.to_string())?,
            WindowPosition::Coordinates { x, y } => window
                .set_position(PhysicalPosition::new(x, y))
                .map_err(|error| error.to_string())?,
        }
    }
    if visible {
        #[cfg(target_os = "macos")]
        window
            .app_handle()
            .show()
            .map_err(|error| error.to_string())?;
        window.show().map_err(|error| error.to_string())?;
        window.unminimize().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        #[cfg(target_os = "macos")]
        if quick {
            for attempt in 0..=20 {
                if window.is_focused().map_err(|error| error.to_string())? {
                    break;
                }
                if attempt == 20 {
                    return Err("快捷粘贴窗口未能获得焦点，请重试".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
        if quick {
            crate::app::set_quick_paste_mode(window, true)?;
        }
    }
    Ok(())
}

#[cfg(desktop)]
fn position_at_cursor(window: &WebviewWindow) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return crate::platform::position_quick_paste_window(window);
    #[cfg(not(target_os = "macos"))]
    {
        let result = (|| -> tauri::Result<()> {
            let cursor = window.cursor_position()?;
            if let Some(monitor) = window.monitor_from_point(cursor.x, cursor.y)? {
                let size = window.outer_size()?;
                let work = monitor.work_area();
                let margin = 8.0 * monitor.scale_factor();
                let gap = 12.0 * monitor.scale_factor();
                let min_x = f64::from(work.position.x) + margin;
                let min_y = f64::from(work.position.y) + margin;
                let max_x = min_x.max(
                    f64::from(work.position.x) + f64::from(work.size.width)
                        - f64::from(size.width)
                        - margin,
                );
                let max_y = min_y.max(
                    f64::from(work.position.y) + f64::from(work.size.height)
                        - f64::from(size.height)
                        - margin,
                );
                let x = if cursor.x + gap <= max_x {
                    cursor.x + gap
                } else {
                    cursor.x - f64::from(size.width) - gap
                };
                let y = if cursor.y + gap <= max_y {
                    cursor.y + gap
                } else {
                    cursor.y - f64::from(size.height) - gap
                };
                window.set_position(PhysicalPosition::new(
                    x.clamp(min_x, max_x).round() as i32,
                    y.clamp(min_y, max_y).round() as i32,
                ))?;
            }
            Ok(())
        })();
        // Preserve the desktop fallback: a positioning failure need not prevent opening.
        if let Err(error) = result {
            log::warn!("定位快捷粘贴窗口失败：{error}");
        }
        Ok(())
    }
}
