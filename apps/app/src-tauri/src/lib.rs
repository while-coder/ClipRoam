mod app;
mod branding;
mod autostart;
mod clipboard;
mod files;
mod platform;
mod platforms;
mod window_mode;
#[path = "../share/mod.rs"]
mod share;
use tauri::Manager;


#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let builder = tauri::Builder::default();
    let builder = builder
        .plugin(logging_plugin())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::Builder::new().open_js_links_on_click(false).build())
        .plugin(tauri_plugin_dialog::init());
    let builder = platform::register_plugins(builder);
    builder
        .setup(|app| {
            #[cfg(target_os = "android")]
            share::attach(app.handle().clone());
            std::fs::create_dir_all(app.path().app_data_dir()?)?;
            app.manage(app::QuickPasteMode::default());
            app.manage(window_mode::WindowModeState::default());
            app.manage(clipboard::ClipboardState::default());
            platform::manage_platform_state(app.handle())?;
            platform::create_windows(app.handle())?;
            #[cfg(desktop)]
            platform::setup_desktop_shell(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            platform::on_window_event(window, event);
        })
        .invoke_handler(tauri::generate_handler![
            autostart::autostart_is_enabled,
            autostart::autostart_set_enabled,
            app::native_get_platform_capabilities,
            app::native_get_device_info,
            app::native_open_app_data_dir,
            app::native_debug_tools_supported,
            app::native_open_devtools,
            app::native_start_window_drag,
            window_mode::native_set_window_mode,
            app::native_capture_paste_target,
            app::native_register_quick_paste_shortcut,
            app::native_show_notification,
            clipboard::native_read_clipboard,
            clipboard::native_write_clipboard,
            clipboard::native_watch_clipboard,
            files::native_paths,
            files::native_fs_stat,
            files::native_fs_list,
            files::native_read_image,
            files::native_fs_read,
            files::native_fs_write,
            files::native_fs_move,
            files::native_fs_copy,
            files::native_fs_mkdir,
            files::native_fs_remove,
            files::native_save_dialog,
            files::native_pick_files,
            share::native_pending_shares,
            share::native_configure_share_session,
            share::native_ack_share,
        ])
        .build(tauri::generate_context!())
        .expect("error while building ClipRoam")
        .run(|_app, _event| {
            // 程序坞点击与托盘点击使用同一恢复流程，包含取消最小化及退出快捷粘贴模式。
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                if let Err(error) = platform::show_main_window(_app) {
                    log::warn!("[window] failed to restore main window from Dock: {error}");
                }
            }
        });
}

/// 构建日志插件：
/// - 三路输出：stdout（dev 控制台）、webview（前端 devtools console）、文件（系统日志目录）；
/// - dev 记到 Debug，release 记到 Info，并压低三方库噪声；
/// - 本地时区时间戳，单文件超 10 MB 轮转，保留最近 3 份历史归档（外加当前活动文件，最多 4 个），避免无限增长。
///   日志文件位置（Windows）：%LOCALAPPDATA%/<bundle-id>/logs/。
fn logging_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};

    let level = if cfg!(debug_assertions) {
        tauri_plugin_log::log::LevelFilter::Debug
    } else {
        tauri_plugin_log::log::LevelFilter::Info
    };

    tauri_plugin_log::Builder::new()
        .level(level)
        // 三方库默认只在出问题时才需要，压到 Warn 减少噪声
        .level_for("tao", tauri_plugin_log::log::LevelFilter::Warn)
        .level_for("reqwest", tauri_plugin_log::log::LevelFilter::Warn)
        .targets([
            Target::new(TargetKind::Stdout),
            Target::new(TargetKind::Webview),
            Target::new(TargetKind::LogDir { file_name: None }),
        ])
        .timezone_strategy(TimezoneStrategy::UseLocal)
        .max_file_size(10_000_000)
        .rotation_strategy(RotationStrategy::KeepSome(3))
        .build()
}
