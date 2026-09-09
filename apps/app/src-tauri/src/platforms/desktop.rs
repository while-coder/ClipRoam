//! Windows/macOS/Linux 三个桌面平台共享的外壳与粘贴投递逻辑。
//! 个别系统差异（粘贴合成、打开数据目录的命令）由各系统目录覆盖。

// `run_paste_command` 与 `begin_window_drag` 只被 macOS/Linux 路径使用，
// Windows 构建下它们由 platforms/windows 覆盖，因此是预期中的死代码。
#![cfg_attr(target_os = "windows", allow(dead_code))]

use std::path::PathBuf;
use std::{thread, time::Duration};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, Position, Size, Window, WindowEvent};

use crate::clipboard::capture::ShareImportSummary;

const TRAY_SHOW_MAIN: &str = "show-main";
const TRAY_QUIT: &str = "quit";

pub(crate) fn register_plugins(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    builder
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_process::init())
}

// ---------------------------------------------------------------------------
// 粘贴投递
// ---------------------------------------------------------------------------

pub(crate) fn run_paste_command(program: &str, arguments: &[&str]) -> Result<(), String> {
    let output = std::process::Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if message.is_empty() {
        format!("{program} 退出码：{}", output.status)
    } else {
        format!("{program}: {message}")
    })
}

pub(crate) fn deliver_paste(window: &tauri::WebviewWindow, synthesize: bool) -> Result<(), String> {
    if !synthesize {
        return Ok(());
    }
    window.hide().map_err(|error| error.to_string())?;
    thread::sleep(Duration::from_millis(90));
    if let Err(error) = crate::platforms::synthesize_paste() {
        // The clipboard content is still valid, but the user needs to see why
        // automatic delivery failed (for example missing Linux helpers or
        // macOS Accessibility permission).
        let _ = window.show();
        return Err(error);
    }
    Ok(())
}

pub(crate) fn requires_paste_window() -> bool {
    true
}

/// 各桌面系统默认不按剪贴板序列号跳过轮询；Windows 有快速路径覆盖。
pub(crate) fn should_skip_clipboard_poll(_sequence: &mut u32) -> bool {
    false
}

/// 虚拟文件粘贴目前只有 Windows 实现；其余桌面平台走物化路径。
pub(crate) fn supports_virtual_file_paste(_entry: &crate::content::ClipboardEntry) -> bool {
    false
}

pub(crate) fn set_virtual_file_clipboard(
    _app: &AppHandle,
    _window_label: &str,
    _entry: crate::content::ClipboardEntry,
) -> Result<(), String> {
    Err("当前平台不支持虚拟文件粘贴".to_string())
}

pub(crate) fn begin_window_drag(window: &tauri::WebviewWindow) -> Result<(), String> {
    window.start_dragging().map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// 剪贴板焦点跟踪（Windows 的守卫版本由 platforms/windows 覆盖）
// ---------------------------------------------------------------------------

pub(crate) fn on_paste_window_focus(app: AppHandle, focused: bool) {
    if focused {
        return;
    }
    spawn_paste_focus_loss_check(app);
}

pub(crate) fn spawn_paste_focus_loss_check(app: AppHandle) {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        let Some(window) = app.get_webview_window("paste") else {
            return;
        };
        if crate::platforms::should_ignore_paste_focus_loss(&app) {
            return;
        }
        if matches!(window.is_focused(), Ok(false)) {
            let _ = window.hide();
        }
    });
}

pub(crate) fn should_ignore_paste_focus_loss(_app: &AppHandle) -> bool {
    false
}

pub(crate) fn on_window_event(window: &Window, event: &WindowEvent) {
    match event {
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = window.hide();
        }
        WindowEvent::Focused(focused) if window.label() == "paste" => {
            crate::platforms::on_paste_window_focus(window.app_handle().clone(), *focused);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// 应用外壳：建窗、托盘、toast
// ---------------------------------------------------------------------------

pub(crate) fn create_windows(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    for window_config in app.config().app.windows.clone() {
        tauri::WebviewWindowBuilder::from_config(app, &window_config)?.build()?;
    }
    Ok(())
}

pub(crate) fn setup_desktop_shell(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    setup_tray(app)?;
    if let Some(window) = app.get_webview_window("toast") {
        let _ = window.set_ignore_cursor_events(true);
    }
    Ok(())
}

fn show_main_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
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

pub(crate) fn show_detached_toast(
    app: &AppHandle,
    payload: crate::app_shell::ToastPayload,
) -> Result<(), String> {
    let window = app
        .get_webview_window("toast")
        .ok_or_else(|| "toast window is unavailable".to_string())?;
    position_toast_window(app, &window)?;
    window
        .emit("cliproam://toast", payload)
        .map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())
}

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

// ---------------------------------------------------------------------------
// 数据目录与另存对话框
// ---------------------------------------------------------------------------

pub(crate) fn open_data_directory_with(app: &AppHandle, command: &str) -> Result<(), String> {
    use std::{fs, process::Command};

    let app_data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    fs::create_dir_all(&app_data_dir).map_err(|error| error.to_string())?;
    Command::new(command)
        .arg(&app_data_dir)
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) fn supports_native_file_export() -> bool {
    true
}

pub(crate) fn prompt_save_destination(single_file: bool, file_name: &str) -> Option<PathBuf> {
    if single_file {
        rfd::FileDialog::new()
            .set_file_name(file_name)
            .save_file()
    } else {
        rfd::FileDialog::new().pick_folder()
    }
}

pub(crate) fn consume_pending_shares(_app: &AppHandle) -> Result<ShareImportSummary, String> {
    Ok(ShareImportSummary::default())
}
