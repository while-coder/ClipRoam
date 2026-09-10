//! 同步账号配置与它选定的历史档案。
//!
//! 保存配置只更新 Rust 侧状态，不向前端发事件：前端每个保存点都显式
//! 决定连接/断开，避免「显式 startSync + 事件回调 startSync」的竞态双连。

use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use tauri::State;

use crate::store::{history_path_for_key, load_history, save_metadata, LOCAL_HISTORY_KEY};
use crate::AppState;

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
    /// 捕获文件/文件夹时按名称跳过的过滤模式（`*`/`?` 通配，如 `node_modules`）。
    #[serde(default = "default_exclude_patterns")]
    pub exclude_patterns: Vec<String>,
    /// 登录时服务器下发的单文件存储上限（MB）；Rust 侧只透传持久化，前端用它
    /// 约束自动上传档位。
    #[serde(default = "default_server_max_file_mb")]
    pub server_max_file_mb: u64,
}

// The serde defaults below must stay in step with the DEFAULT_* constants in
// packages/protocol/src/index.ts, which are what the frontend actually shows.
fn default_server_protocol() -> String {
    "http".to_string()
}

fn default_auto_upload_limit_mb() -> u64 {
    50
}

fn default_auto_receive_clipboard() -> bool {
    true
}

fn default_exclude_patterns() -> Vec<String> {
    vec!["node_modules".to_string()]
}

fn default_server_max_file_mb() -> u64 {
    200
}

/// 配置对应的本地历史档案键：登录账号用 `account:{服务器}:{用户名}`，
/// 未登录回落到本地档案。
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

/// 保存配置；键变化时切换到新档案的历史库（设备身份带过去）。
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn save_sync_config(
    state: State<'_, AppState>,
    config: SyncConfig,
) -> Result<(), String> {
    let history_key = history_key_for_config(&config);
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        if history.active_history != history_key {
            // Persist the outgoing profile's metadata before switching to it.
            let current_path = history_path_for_key(&state.histories_dir, &history.active_history);
            state.with_database(&current_path, |connection| save_metadata(connection, &history))?;
            let next_path = history_path_for_key(&state.histories_dir, &history_key);
            let profile_exists = next_path.exists();
            let device_id = history.device_id.clone();
            let device_name = history.device_name.clone();
            let mut next_history = load_history(&next_path, &history_key);
            if !profile_exists {
                next_history.device_id = device_id;
                next_history.device_name = device_name;
            }
            *history = next_history;
        }
        // A fresh profile has no device identity row yet; the metadata write
        // lands it.
        let active_path = history_path_for_key(&state.histories_dir, &history.active_history);
        state.with_database(&active_path, |connection| save_metadata(connection, &history))?;
    }
    let config = Some(config);
    write_sync_config(&state.sync_config_path, &config)?;
    *state.sync_config.lock().map_err(|error| error.to_string())? = config;
    Ok(())
}
