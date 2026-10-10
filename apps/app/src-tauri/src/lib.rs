mod app_shell;
mod content;
mod history;
mod file;
mod logging;
mod pending_upload;
mod platforms;
mod store;
mod sync;
mod downloads;
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
    cache_dir_for, history_path_for_key, load_history, preferences_path_for, DatabasePool,
    HistoryData,
};
use sync::SyncConfig;
use downloads::{DownloadState, Downloader, VirtualDownloads};
use downloads::save::SaveSession;

struct AppState {
    history: Mutex<HistoryData>,
    /// Machine-level device identity file (`device.json`), shared by every
    /// history profile; resolved lazily by `get_device_id`.
    device_config_path: PathBuf,
    histories_dir: PathBuf,
    /// One SQLite connection per history database; opening one re-runs the
    /// schema migration, so writes share these instead of reopening.
    database_pool: Mutex<DatabasePool>,
    sync_config: Mutex<Option<SyncConfig>>,
    sync_config_path: PathBuf,
    /// Preferences of the active history profile (`preferences.json` next to
    /// its SQLite); reloaded whenever the active profile changes.
    account_preferences: Mutex<sync::AccountPreferences>,
    downloads: Mutex<HashMap<String, DownloadState>>,
    save_sessions: Mutex<HashMap<String, SaveSession>>,
    virtual_downloads: VirtualDownloads,
    /// 全局下载管理器：队列/并发/去重/拉流都在这里（main 与 paste 共享）。
    downloader: Downloader,
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

    /// 活动档案的数据库路径；未登录时没有档案可用。
    pub(crate) fn active_history_path(&self, history: &HistoryData) -> Result<PathBuf, String> {
        let key = history
            .active_history
            .as_deref()
            .ok_or("同步账号未登录，历史档案不可用")?;
        Ok(history_path_for_key(&self.histories_dir, key))
    }

    /// 活动档案的内容缓存目录；未登录时没有档案可用。
    pub(crate) fn active_cache_dir(&self, history: &HistoryData) -> Result<PathBuf, String> {
        let key = history
            .active_history
            .as_deref()
            .ok_or("同步账号未登录，历史档案不可用")?;
        Ok(cache_dir_for(&self.histories_dir, key))
    }

    /// 设置页「虚拟文件粘贴」开关（Windows 粘贴策略每次调用时读取）；
    /// 锁中毒时按默认关闭——物化路径在所有平台都可用。
    pub(crate) fn use_virtual_files(&self) -> bool {
        self.account_preferences
            .lock()
            .map(|preferences| preferences.use_virtual_files)
            .unwrap_or(false)
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // reqwest uses rustls-no-provider, so register ring before any HTTP client is created.
    let _ = rustls::crypto::ring::default_provider().install_default();
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
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init());
    let builder = platforms::register_plugins(builder);
    let builder = tauri_updater_kit::attach_updater(builder);

    let builder = builder
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
            let histories_dir = app_data_dir.join("histories");
            // 数据目录不预建的话，首次登录写 sync-config.json 会直接 ENOENT。
            std::fs::create_dir_all(&histories_dir).map_err(|error| error.to_string())?;
            let sync_config_path = app_data_dir.join("sync-config.json");
            let device_config_path = app_data_dir.join("device.json");
            let sync_config = sync::load_sync_config(&sync_config_path);
            // 登出只清 token、保留配置；无 token 即未登录，没有活动档案。
            let history_key = sync_config
                .as_ref()
                .and_then(sync::signed_in_config)
                .map(sync::history_key_for_config);
            // 登录态决定活动档案：未登录时没有档案，捕获与查询都不可用。
            let (history, account_preferences) = match &history_key {
                Some(key) => {
                    let history_path = history_path_for_key(&histories_dir, key);
                    let history = load_history(&history_path, key);
                    // 偏好跟随活动档案：缺失或损坏时回落默认值。
                    let preferences_path = preferences_path_for(&histories_dir, key);
                    let account_preferences = sync::load_preferences(&preferences_path);
                    (history, account_preferences)
                }
                None => (HistoryData::default(), sync::AccountPreferences::default()),
            };
            app.manage(AppState {
                history: Mutex::new(history),
                device_config_path,
                histories_dir,
                database_pool: Mutex::new(DatabasePool::default()),
                sync_config: Mutex::new(sync_config),
                sync_config_path,
                account_preferences: Mutex::new(account_preferences),
                downloads: Mutex::new(HashMap::new()),
                save_sessions: Mutex::new(HashMap::new()),
                virtual_downloads: VirtualDownloads::default(),
                downloader: Downloader::new(app.handle().clone()),
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
                // 未登录时没有活动档案，也没有可回收的引用。
                let Some(key) = history.active_history.clone() else {
                    return;
                };
                let path = history_path_for_key(&state.histories_dir, &key);
                let cache_dir = cache_dir_for(&state.histories_dir, &key);
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
            pending_upload::capture::capture_current_clipboard_text,
            pending_upload::capture::capture_files_from_picker,
            pending_upload::capture::consume_mobile_shares,
            history::query::get_cached_entries_for_display,
            history::query::find_stale_entry_ids,
            history::query::get_entry,
            file::store::find_unknown_file_ids,
            file::store::upsert_server_files,
            downloads::list_entry_files,
            app_shell::get_device_identity,
            app_shell::save_device_alias,
            app_shell::get_device_info,
            sync::get_sync_config,
            sync::get_account_preferences,
            app_shell::open_app_data_dir,
            sync::save_sync_config,
            sync::save_account_preferences,
            history::mutate::upsert_server_entries,
            history::mutate::remove_server_entry,
            pending_upload::peek_pending_entry,
            pending_upload::dequeue_pending_entry,
            pending_upload::count_pending_entries,
            pending_upload::list_pending_entries,
            app_shell::start_window_drag,
            app_shell::hide_paste,
            app_shell::capture_paste_target,
            app_shell::hide_main,
            app_shell::show_toast,
            app_shell::hide_toast,
            downloads::prepare_entry_files,
            downloads::prepare_paste_entry,
            downloads::save::prepare_save_entry,
            file::read::read_upload_chunk,
            downloads::download_files,
            downloads::cancel_download,
            downloads::cancel_entry_downloads,
            downloads::stop_all_downloads,
            downloads::download_tasks,
            downloads::save::cancel_save_entry,
            downloads::save::finish_save_entry,
            history::clipboard::activate_remote_entry,
            history::clipboard::copy_entry,
            history::clipboard::paste_entry
        ])
        .run(tauri::generate_context!())
        .expect("error while running ClipRoam");
}
