//! 同步账号配置与它选定的历史档案。
//!
//! 保存配置只更新 Rust 侧状态，不向前端发事件：前端每个保存点都显式
//! 决定连接/断开，避免「显式 startSync + 事件回调 startSync」的竞态双连。

use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use tauri::State;

use crate::store::{history_path_for_key, load_history, save_metadata, LOCAL_HISTORY_KEY};
use crate::utils::write_file_atomic;
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
    /// 连接后拉取同步历史的每页数量（10-100）；Rust 侧只透传持久化。
    #[serde(default = "default_manifest_page_size")]
    pub manifest_page_size: u32,
    /// 单次复制文件数上限（登录时服务器下发的 settings.maxCaptureFileCount）；
    /// 捕获时超过则不捕获、不同步。
    #[serde(default = "default_max_capture_file_count")]
    pub max_capture_file_count: u64,
}

// The serde defaults below must stay in step with the app-side defaults in
// apps/app/src/features/sync/syncDefaults.ts, which is what the frontend shows.
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

fn default_manifest_page_size() -> u32 {
    100
}

pub(crate) fn default_max_capture_file_count() -> u64 {
    1000
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
    // 原子写：直接覆盖的话，写一半崩溃/断电会留下无法解析的配置，下次启动
    // 回落到本地档案——用户视角等于账号历史全部「消失」。
    write_file_atomic(path, json.as_bytes())
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
/// 顺序很讲究：先持久化旧档案元数据，再原子写配置文件，最后才切换内存
/// 档案。配置写失败时直接返回，内存档案原封不动——不会出现「内存已切到
/// 账号档案、磁盘配置还是本地」的分裂状态。
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn save_sync_config(
    state: State<'_, AppState>,
    config: SyncConfig,
) -> Result<(), String> {
    let history_key = history_key_for_config(&config);
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    if history.active_history != history_key {
        // Persist the outgoing profile's metadata before leaving it.
        let current_path = history_path_for_key(&state.histories_dir, &history.active_history);
        state.with_database(&current_path, |connection| save_metadata(connection, &history))?;
    }
    write_sync_config(&state.sync_config_path, &Some(config.clone()))?;
    if history.active_history != history_key {
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
    *state.sync_config.lock().map_err(|error| error.to_string())? = Some(config);
    Ok(())
}
