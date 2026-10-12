//! Immutable account resources shared by commands and their background work.
use rusqlite::Connection;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{
    store::{cache_dir_for, history_path_for_key, preferences_path_for, load_history, save_metadata, HistoryData},
    sync::{history_key_for_config, SyncConfig},
    AppState,
};

pub(crate) struct AccountContext {
    pub key: String,
    database: Arc<Mutex<Connection>>,
    pub cache_dir: PathBuf,
    pub preferences_path: PathBuf,
    pub config: SyncConfig,
    closed: AtomicBool,
}

impl AccountContext {
    pub fn new(state: &AppState, config: SyncConfig) -> Result<Self, String> {
        let key = history_key_for_config(&config);
        Ok(Self {
            database: state
                .database_connection(&history_path_for_key(&state.histories_dir, &key))?,
            cache_dir: cache_dir_for(&state.histories_dir, &key),
            preferences_path: preferences_path_for(&state.histories_dir, &key),
            key,
            config,
            closed: AtomicBool::new(false),
        })
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub fn with_database<T>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut connection = self.database.lock().map_err(|error| error.to_string())?;
        if self.is_closed() { return Err("账号会话已结束".to_string()); }
        work(&mut connection)
    }
}

impl AppState {
    pub(crate) fn deactivate_account(&self) -> Result<(), String> {
        crate::pending_upload::capture::stop_clipboard_monitor(self);
        let mut history = self.history.lock().map_err(|error| error.to_string())?;
        let saved = if let Some(key) = &history.active_history {
            let path = history_path_for_key(&self.histories_dir, key);
            self.with_database(&path, |connection| save_metadata(connection, &history))
        } else { Ok(()) };
        *history = HistoryData::default();
        *self.account_preferences.lock().map_err(|error| error.to_string())? = crate::sync::AccountPreferences::default();
        for (_, account) in self.account_sessions.lock().map_err(|error| error.to_string())?.drain() {
            account.closed.store(true, Ordering::Release);
        }
        self.downloader.stop_all("同步账号未登录");
        saved
    }

    pub(crate) fn account(&self, session_id: &str) -> Result<Arc<AccountContext>, String> {
        self.account_sessions
            .lock()
            .map_err(|error| error.to_string())?
            .get(session_id)
            .cloned()
            .ok_or_else(|| "账号会话已结束".to_string())
    }
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn open_account_session(
    state: State<'_, AppState>,
    session_id: String,
    config: SyncConfig,
) -> Result<(), String> {
    if crate::sync::signed_in_config(&config).is_none() {
        return Err("同步账号未登录".to_string());
    }
    let history = state.history.lock().map_err(|error| error.to_string())?;
    if history.active_history.as_deref() != Some(history_key_for_config(&config).as_str()) {
        return Err("同步账号未登录".to_string());
    }
    let account = Arc::new(AccountContext::new(&state, config)?);
    let mut sessions = state
        .account_sessions
        .lock()
        .map_err(|error| error.to_string())?;
    if sessions.contains_key(&session_id) {
        return Err("账号会话已存在".to_string());
    }
    sessions.insert(session_id, account);
    Ok(())
}

/// 仅由主窗口在 login/verify 成功后调用；配置读取和持久化不会启用账号。
#[tauri::command]
pub(crate) fn activate_account(app: AppHandle, window: tauri::WebviewWindow, state: State<'_, AppState>) -> Result<(), String> {
    if window.label() != "main" { return Err("仅主窗口可启用账号".to_string()); }
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    let config = state.sync_config.lock().map_err(|error| error.to_string())?.clone()
        .filter(|config| crate::sync::signed_in_config(config).is_some())
        .ok_or("同步账号未登录")?;
    let key = history_key_for_config(&config);
    *history = load_history(&history_path_for_key(&state.histories_dir, &key), &key);
    *state.account_preferences.lock().map_err(|error| error.to_string())? =
        crate::sync::load_preferences(&preferences_path_for(&state.histories_dir, &key));
    drop(history);
    crate::platforms::start_clipboard_monitor(app.clone());
    let _ = app.emit_to("paste", "cliproam://sync-config-changed", ());
    Ok(())
}

#[tauri::command]
pub(crate) fn deactivate_account(app: AppHandle, window: tauri::WebviewWindow, state: State<'_, AppState>) -> Result<(), String> {
    if window.label() != "main" { return Err("仅主窗口可停用账号".to_string()); }
    state.deactivate_account()?;
    if let Some(paste) = app.get_webview_window("paste") { let _ = paste.hide(); }
    let _ = app.emit_to("paste", "cliproam://sync-config-changed", ());
    Ok(())
}

#[tauri::command]
pub(crate) fn get_authenticated_sync_config(state: State<'_, AppState>) -> Result<Option<SyncConfig>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let config = state.sync_config.lock().map_err(|error| error.to_string())?;
    Ok(config.as_ref().filter(|config| history.active_history.as_deref() == Some(history_key_for_config(config).as_str())).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_account_rejects_late_database_work() {
        let database = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        database.lock().unwrap().execute_batch("CREATE TABLE entries (content TEXT)").unwrap();
        let account = AccountContext {
            key: "account:test:user".to_string(),
            database: database.clone(),
            cache_dir: PathBuf::new(),
            preferences_path: PathBuf::new(),
            config: SyncConfig {
                server_address: "localhost:4810".to_string(),
                server_protocol: "http".to_string(),
                username: "user".to_string(),
                session_token: "saved-token".to_string(),
            },
            closed: AtomicBool::new(false),
        };
        account.with_database(|connection| {
            connection.execute("INSERT INTO entries VALUES ('before logout')", []).map_err(|error| error.to_string())
        }).unwrap();
        account.closed.store(true, Ordering::Release);
        assert!(account.with_database(|connection| {
            connection.execute("INSERT INTO entries VALUES ('after logout')", []).map_err(|error| error.to_string())
        }).is_err());
        let count: i64 = database.lock().unwrap().query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 1);
    }
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn close_account_session(
    state: State<'_, AppState>,
    session_id: String,
    stop_downloads: bool,
) -> Result<(), String> {
    let account = state
        .account_sessions
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&session_id);
    if let Some(account) = account {
        account.closed.store(true, Ordering::Release);
        if stop_downloads {
            state
                .downloader
                .stop_account(&account, Some(&session_id), "账号会话已结束");
        }
    }
    // Already-dispatched work retains its Arc and always uses the original database.
    Ok(())
}
