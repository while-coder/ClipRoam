//! Immutable account resources shared by commands and their background work.
use rusqlite::Connection;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::State;

use crate::{
    store::{cache_dir_for, history_path_for_key, preferences_path_for},
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
        work(&mut connection)
    }
}

impl AppState {
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
