//! Sync account configuration and the history profile it selects.

use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use tauri::{AppHandle, Emitter, State};

use crate::store::{history_path_for_key, load_history, retain_single_history, LOCAL_HISTORY_KEY};
use crate::{save_active_history, AppState};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncConfig {
    pub enabled: bool,
    #[serde(default, alias = "serverUrl")]
    pub server_address: String,
    #[serde(default = "default_server_protocol")]
    pub server_protocol: String,
    #[serde(default)]
    pub username: String,
    #[serde(default, alias = "token")]
    pub session_token: String,
    #[serde(default = "default_auto_upload_limit_mb")]
    pub auto_upload_limit_mb: u64,
    #[serde(default = "default_auto_receive_clipboard")]
    pub auto_receive_clipboard: bool,
}

// The serde defaults below must stay in step with the DEFAULT_* constants in
// packages/protocol/src/index.ts, which are what the frontend actually shows.
fn default_server_protocol() -> String {
    "http".to_string()
}

fn default_auto_upload_limit_mb() -> u64 {
    10
}

fn default_auto_receive_clipboard() -> bool {
    true
}

pub(crate) fn history_key_for_config(config: &SyncConfig) -> String {
    if config.enabled && !config.username.trim().is_empty() {
        format!(
            "account:{}:{}",
            config.server_address.trim().to_ascii_lowercase(),
            config.username.trim().to_ascii_lowercase()
        )
    } else {
        LOCAL_HISTORY_KEY.to_string()
    }
}

pub(crate) fn load_sync_config(path: &Path) -> Option<SyncConfig> {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

pub(crate) fn write_sync_config(path: &Path, config: &Option<SyncConfig>) -> Result<(), String> {
    let json = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn get_sync_config(state: State<'_, AppState>) -> Result<Option<SyncConfig>, String> {
    Ok(state
        .sync_config
        .lock()
        .map_err(|error| error.to_string())?
        .clone())
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn save_sync_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: SyncConfig,
) -> Result<(), String> {
    let history_key = history_key_for_config(&config);
    let pending = {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        if history.active_history != history_key {
            save_active_history(&state, &history)?;
            let next_path = history_path_for_key(&state.histories_dir, &history_key);
            let profile_exists = next_path.exists();
            let device_id = history.device_id.clone();
            let device_name = history.device_name.clone();
            let mut next_history = load_history(&next_path, &history_key);
            retain_single_history(&mut next_history, &history_key);
            if !profile_exists {
                next_history.device_id = device_id;
                next_history.device_name = device_name;
            }
            *history = next_history;
        }
        save_active_history(&state, &history)?;
        crate::clipboard::hashing::pending_entry_ids(&history)
    };
    for entry_id in pending {
        crate::clipboard::hashing::queue_hashing(&state, &entry_id);
    }
    let config = Some(config);
    write_sync_config(&state.sync_config_path, &config)?;
    *state.sync_config.lock().map_err(|error| error.to_string())? = config;
    app.emit("cliproam://sync-config-changed", ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::SyncConfig;

    #[test]
    fn older_sync_config_enables_clipboard_roaming_by_default() {
        let config: SyncConfig = serde_json::from_str(
            r#"{
                "enabled": true,
                "serverAddress": "127.0.0.1:4810",
                "serverProtocol": "http",
                "username": "tester",
                "sessionToken": "token",
                "autoUploadLimitMb": 10
            }"#,
        )
        .unwrap();

        assert!(config.auto_receive_clipboard);
    }
}
