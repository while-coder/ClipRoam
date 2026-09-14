//! 同步账号配置与它选定的历史档案。
//!
//! 保存配置只更新 Rust 侧状态，不向前端发事件：前端每个保存点都显式
//! 决定连接/断开，避免「显式 startSync + 事件回调 startSync」的竞态双连。

use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use tauri::State;

use crate::store::{
    history_path_for_key, load_history, preferences_path_for, save_metadata, LOCAL_HISTORY_KEY,
};
use crate::utils::write_file_atomic;
use crate::AppState;

/// 会话凭证：全局唯一（`sync-config.json`），决定本次启动激活哪个账号档案。
/// 客户端偏好与服务器限额跟档案走，见 [`AccountPreferences`]。
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
}

/// 账号偏好：跟随历史档案存在档案目录的 `preferences.json`，换账号登录时
/// 互不污染；未登录的本地档案也有一份（捕获过滤与限额仍生效）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct AccountPreferences {
    #[serde(default = "default_auto_upload_limit_mb")]
    pub auto_upload_limit_mb: u64,
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

impl Default for AccountPreferences {
    fn default() -> Self {
        Self {
            auto_upload_limit_mb: default_auto_upload_limit_mb(),
            auto_receive_clipboard: default_auto_receive_clipboard(),
            exclude_patterns: default_exclude_patterns(),
            server_max_file_mb: default_server_max_file_mb(),
            manifest_page_size: default_manifest_page_size(),
            max_capture_file_count: default_max_capture_file_count(),
        }
    }
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

fn default_max_capture_file_count() -> u64 {
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

pub(crate) fn load_preferences(path: &Path) -> AccountPreferences {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub(crate) fn write_preferences(
    path: &Path,
    preferences: &AccountPreferences,
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(preferences).map_err(|error| error.to_string())?;
    write_file_atomic(path, json.as_bytes())
}

/// 启动时确保活动档案有 preferences.json：缺失则把旧 sync-config.json 里
/// 的偏好值（无则默认值）写入，并把配置重写为纯会话字段；已存在则直接
/// 读取。返回本次启动生效的偏好。
pub(crate) fn migrate_preferences(
    preferences_path: &Path,
    sync_config_path: &Path,
) -> Result<AccountPreferences, String> {
    if preferences_path.exists() {
        return Ok(load_preferences(preferences_path));
    }
    let preferences = load_legacy_preferences(sync_config_path).unwrap_or_default();
    write_preferences(preferences_path, &preferences)?;
    if let Some(config) = load_sync_config(sync_config_path) {
        // 重写剥离混在配置里的旧偏好字段；失败无碍，读取时多余字段会被忽略。
        let _ = write_sync_config(sync_config_path, &Some(config));
    }
    Ok(preferences)
}

/// 一次性迁移：拆分前的 sync-config.json 把偏好字段和会话字段混在一起，
/// 这里按旧字段全集只解析出偏好。文件缺失或损坏返回 None。
fn load_legacy_preferences(path: &Path) -> Option<AccountPreferences> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LegacyFields {
        #[serde(default = "default_auto_upload_limit_mb")]
        auto_upload_limit_mb: u64,
        #[serde(default = "default_auto_receive_clipboard")]
        auto_receive_clipboard: bool,
        #[serde(default = "default_exclude_patterns")]
        exclude_patterns: Vec<String>,
        #[serde(default = "default_server_max_file_mb")]
        server_max_file_mb: u64,
        #[serde(default = "default_manifest_page_size")]
        manifest_page_size: u32,
        #[serde(default = "default_max_capture_file_count")]
        max_capture_file_count: u64,
    }

    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<LegacyFields>(&raw).ok())
        .map(|legacy| AccountPreferences {
            auto_upload_limit_mb: legacy.auto_upload_limit_mb,
            auto_receive_clipboard: legacy.auto_receive_clipboard,
            exclude_patterns: legacy.exclude_patterns,
            server_max_file_mb: legacy.server_max_file_mb,
            manifest_page_size: legacy.manifest_page_size,
            max_capture_file_count: legacy.max_capture_file_count,
        })
}

#[tauri::command]
pub(crate) fn get_sync_config(state: State<'_, AppState>) -> Result<Option<SyncConfig>, String> {
    Ok(state
        .sync_config
        .lock()
        .map_err(|error| error.to_string())?
        .clone())
}

#[tauri::command]
pub(crate) fn get_account_preferences(
    state: State<'_, AppState>,
) -> Result<AccountPreferences, String> {
    Ok(state
        .account_preferences
        .lock()
        .map_err(|error| error.to_string())?
        .clone())
}

/// 保存偏好到当前活动档案：preferences.json 与档案同生共死，不进全局配置。
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn save_account_preferences(
    state: State<'_, AppState>,
    preferences: AccountPreferences,
) -> Result<(), String> {
    let path = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        preferences_path_for(&state.histories_dir, &history.active_history)
    };
    write_preferences(&path, &preferences)?;
    *state
        .account_preferences
        .lock()
        .map_err(|error| error.to_string())? = preferences;
    Ok(())
}

/// 保存配置；键变化时切换到新档案的历史库。设备身份是机器级的
/// （`device.json`），不随档案切换。
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
        let next_history = load_history(&next_path, &history_key);
        *history = next_history;
        // 偏好跟随档案：切到新档案后加载它的 preferences.json。
        let preferences_path = preferences_path_for(&state.histories_dir, &history_key);
        let preferences = load_preferences(&preferences_path);
        *state
            .account_preferences
            .lock()
            .map_err(|error| error.to_string())? = preferences;
    }
    let active_path = history_path_for_key(&state.histories_dir, &history.active_history);
    state.with_database(&active_path, |connection| save_metadata(connection, &history))?;
    *state.sync_config.lock().map_err(|error| error.to_string())? = Some(config);
    Ok(())
}
