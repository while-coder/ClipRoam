mod app_shell;
mod clipboard;
mod content;
mod history;
mod platforms;
mod store;
mod sync;
mod transfer;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{mpsc, Mutex},
    thread,
};
use tauri::Manager;

use store::{
    cache_dir_for, collect_local_garbage, default_active_history, history_path_for_key,
    load_history, retain_single_history, save_history, HistoryData,
};
use sync::config::SyncConfig;
use transfer::download::{DownloadState, VirtualDownloads};
use transfer::save::SaveSession;

struct AppState {
    history: Mutex<HistoryData>,
    histories_dir: PathBuf,
    sync_config: Mutex<Option<SyncConfig>>,
    sync_config_path: PathBuf,
    downloads: Mutex<HashMap<String, DownloadState>>,
    save_sessions: Mutex<HashMap<String, SaveSession>>,
    virtual_downloads: VirtualDownloads,
    /// `Sender` is not `Sync`, so managed state has to guard it.
    hash_queue: Mutex<mpsc::Sender<String>>,
    share_import: Mutex<()>,
}

fn save_active_history(
    state: &AppState,
    history: &HistoryData,
) -> Result<(), String> {
    save_history(
        &history_path_for_key(&state.histories_dir, &history.active_history),
        history,
    )
}

fn active_cache_dir(state: &AppState, history: &HistoryData) -> PathBuf {
    cache_dir_for(&state.histories_dir, &history.active_history)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_clipboard_manager::init());
    let builder = platforms::register_plugins(builder);
    let builder = tauri_updater_kit::attach_updater(builder);

    let builder = builder
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
            let histories_dir = app_data_dir.join("histories");
            let sync_config_path = app_data_dir.join("sync-config.json");
            let sync_config = sync::config::load_sync_config(&sync_config_path);
            let history_key = sync_config
                .as_ref()
                .map(sync::config::history_key_for_config)
                .unwrap_or_else(default_active_history);
            let mut history = load_history(&history_path_for_key(&histories_dir, &history_key), &history_key);
            retain_single_history(&mut history, &history_key);
            save_history(&history_path_for_key(&histories_dir, &history_key), &history)?;
            let (sender, receiver) = mpsc::channel::<String>();
            app.manage(AppState {
                history: Mutex::new(history),
                histories_dir,
                sync_config: Mutex::new(sync_config),
                sync_config_path,
                downloads: Mutex::new(HashMap::new()),
                save_sessions: Mutex::new(HashMap::new()),
                virtual_downloads: VirtualDownloads::default(),
                hash_queue: Mutex::new(sender),
                share_import: Mutex::new(()),
            });
            platforms::manage_platform_state(app.handle())?;

            // Desktop windows use `create: false`, so create them after managed
            // state exists. Mobile is a no-op: the system creates the main
            // webview before this hook and rebuilding it would fail with a
            // duplicate label.
            platforms::create_windows(app.handle())?;
            platforms::setup_desktop_shell(app.handle())?;
            clipboard::hashing::start_hash_worker(app.handle().clone(), receiver);
            platforms::start_clipboard_monitor(app.handle().clone());

            // Hashes that were still pending when the app last closed are
            // persisted, so they simply resume.
            let handle = app.handle().clone();
            thread::spawn(move || {
                let state = handle.state::<AppState>();
                let pending = match state.history.lock() {
                    Ok(mut history) => {
                        let _ = collect_local_garbage(&state.histories_dir, &mut history);
                        clipboard::hashing::pending_entry_ids(&history)
                    }
                    Err(_) => Vec::new(),
                };
                for entry_id in pending {
                    clipboard::hashing::queue_hashing(&state, &entry_id);
                }
            });
            Ok(())
        })
        .on_window_event(platforms::on_window_event);

    builder
        .invoke_handler(tauri::generate_handler![
            app_shell::get_platform_capabilities,
            clipboard::capture::capture_current_clipboard_text,
            clipboard::capture::consume_mobile_shares,
            history::list_entries,
            history::get_entry,
            transfer::download::list_entry_files,
            sync::remote::filter_unknown_file_ids,
            history::get_device,
            sync::config::get_sync_config,
            app_shell::open_app_data_dir,
            sync::config::save_sync_config,
            sync::remote::upsert_remote_entry,
            sync::remote::upsert_remote_entries,
            sync::remote::apply_published_entry,
            sync::remote::mark_files_uploaded,
            sync::remote::mark_file_available,
            history::delete_entry,
            history::clear_history,
            sync::remote::remove_remote_entry,
            sync::remote::list_pending_deletions,
            sync::remote::acknowledge_entry_deletion,
            sync::remote::list_pending_entries,
            sync::remote::acknowledge_pending_entry,
            app_shell::start_window_drag,
            app_shell::hide_paste,
            app_shell::hide_main,
            app_shell::show_toast,
            app_shell::hide_toast,
            history::refresh_entry,
            transfer::download::prepare_entry_files,
            transfer::download::prepare_paste_entry,
            transfer::save::prepare_save_entry,
            transfer::download::read_file_chunk,
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
