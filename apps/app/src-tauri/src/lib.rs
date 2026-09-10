mod app_shell;
mod clipboard;
mod content;
mod entry;
mod file;
mod logging;
mod pending;
mod platforms;
mod store;
mod sync;
mod transfer;
mod utils;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
};
use rusqlite::Connection;
use tauri::Manager;

use file::collect_local_garbage;
use store::{
    cache_dir_for, default_active_history, history_path_for_key, load_history, DatabasePool,
    HistoryData,
};
use sync::SyncConfig;
use transfer::download::{DownloadState, VirtualDownloads};
use transfer::save::SaveSession;

struct AppState {
    history: Mutex<HistoryData>,
    histories_dir: PathBuf,
    /// One SQLite connection per history database; opening one re-runs the
    /// schema migration, so writes share these instead of reopening.
    database_pool: Mutex<DatabasePool>,
    sync_config: Mutex<Option<SyncConfig>>,
    sync_config_path: PathBuf,
    downloads: Mutex<HashMap<String, DownloadState>>,
    save_sessions: Mutex<HashMap<String, SaveSession>>,
    virtual_downloads: VirtualDownloads,
    share_import: Mutex<()>,
}

impl AppState {
    /// Runs `write` with the pooled connection of a history database. Lock
    /// order: take this only while already holding `history`, never before it.
    fn with_database<T>(
        &self,
        path: &Path,
        write: impl FnOnce(&mut Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut pool = self.database_pool.lock().map_err(|error| error.to_string())?;
        write(pool.connection(path)?)
    }
}

fn active_cache_dir(state: &AppState, history: &HistoryData) -> PathBuf {
    cache_dir_for(&state.histories_dir, &history.active_history)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 单实例最先注册：第二次启动会自行退出，并唤醒已运行实例的主窗口。
    // 移动端由系统保证单实例，插件也仅支持桌面端。
    #[cfg(desktop)]
    let builder = tauri::Builder::default().plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
        platforms::show_main_window(app);
    }));
    #[cfg(not(desktop))]
    let builder = tauri::Builder::default();

    // 日志插件随后注册，尽量覆盖后续插件与 setup 阶段的日志
    let builder = builder
        .plugin(logging::logging_plugin())
        .plugin(tauri_plugin_clipboard_manager::init());
    let builder = platforms::register_plugins(builder);
    let builder = tauri_updater_kit::attach_updater(builder);

    let builder = builder
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
            let histories_dir = app_data_dir.join("histories");
            let sync_config_path = app_data_dir.join("sync-config.json");
            let sync_config = sync::load_sync_config(&sync_config_path);
            let history_key = sync_config
                .as_ref()
                .map(sync::history_key_for_config)
                .unwrap_or_else(default_active_history);
            let history = load_history(&history_path_for_key(&histories_dir, &history_key), &history_key);
            app.manage(AppState {
                history: Mutex::new(history),
                histories_dir,
                database_pool: Mutex::new(DatabasePool::default()),
                sync_config: Mutex::new(sync_config),
                sync_config_path,
                downloads: Mutex::new(HashMap::new()),
                save_sessions: Mutex::new(HashMap::new()),
                virtual_downloads: VirtualDownloads::default(),
                share_import: Mutex::new(()),
            });
            platforms::manage_platform_state(app.handle())?;

            // Desktop windows use `create: false`, so create them after managed
            // state exists. Mobile is a no-op: the system creates the main
            // webview before this hook and rebuilding it would fail with a
            // duplicate label.
            platforms::create_windows(app.handle())?;
            platforms::setup_desktop_shell(app.handle())?;
            platforms::start_clipboard_monitor(app.handle().clone());

            // Entry rows live in SQLite alone, so startup only frees disk:
            // blobs and share requests no entry references go away.
            let handle = app.handle().clone();
            thread::spawn(move || {
                let state = handle.state::<AppState>();
                let Ok(history) = state.history.lock() else {
                    return;
                };
                let path = history_path_for_key(&state.histories_dir, &history.active_history);
                let cache_dir = active_cache_dir(&state, &history);
                let _ = state.with_database(&path, |connection| {
                    collect_local_garbage(connection, &cache_dir)
                });
            });
            Ok(())
        })
        .on_window_event(platforms::on_window_event);

    builder
        .invoke_handler(tauri::generate_handler![
            app_shell::get_platform_capabilities,
            clipboard::capture::capture_current_clipboard_text,
            clipboard::capture::consume_mobile_shares,
            entry::query::list_entries_manifest,
            entry::query::find_unknown_entry_ids,
            entry::query::get_entry,
            file::query::list_upload_candidates,
            file::store::find_unknown_file_ids,
            file::store::upsert_server_files,
            transfer::download::list_entry_files,
            app_shell::get_device,
            sync::get_sync_config,
            app_shell::open_app_data_dir,
            sync::save_sync_config,
            entry::mutate::upsert_server_entries,
            entry::mutate::remove_server_entry,
            pending::peek_pending_entry,
            pending::dequeue_pending_entry,
            pending::count_pending_entries,
            pending::list_pending_entries,
            app_shell::start_window_drag,
            app_shell::hide_paste,
            app_shell::hide_main,
            app_shell::show_toast,
            app_shell::hide_toast,
            transfer::download::prepare_entry_files,
            transfer::download::prepare_paste_entry,
            transfer::save::prepare_save_entry,
            transfer::download::read_upload_chunk,
            transfer::download::begin_file_download,
            transfer::download::append_file_download,
            transfer::download::finish_file_download,
            transfer::download::cancel_file_download,
            transfer::save::cancel_save_entry,
            transfer::save::finish_save_entry,
            transfer::download::fail_virtual_file_request,
            clipboard::output::activate_remote_entry,
            clipboard::output::copy_entry,
            clipboard::output::paste_entry
        ])
        .run(tauri::generate_context!())
        .expect("error while running ClipRoam");
}
