//! 同步账号配置与它选定的历史档案。
//!
//! 保存配置只更新 Rust 侧状态，不向前端发事件：前端每个保存点都显式
//! 决定连接/断开，避免「显式 startSync + 事件回调 startSync」的竞态双连。

use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use tauri::State;

use crate::store::{
    history_path_for_key, load_history, preferences_path_for, save_metadata, HistoryData,
};
use crate::utils::write_file_atomic;
use crate::AppState;

mod token_store;

/// 会话凭证：全局唯一（`sync-config.json`），决定本次启动激活哪个账号档案。
/// 客户端偏好与服务器限额跟档案走，见 [`AccountPreferences`]。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncConfig {
    #[serde(default)]
    pub server_address: String,
    #[serde(default = "default_server_protocol")]
    pub server_protocol: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub session_token: String,
}

/// 账号偏好：跟随历史档案存在档案目录的 `preferences.json`，换账号登录时
/// 互不污染；未登录的默认档案也有一份（捕获过滤与限额仍生效）。
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

/// 配置对应的历史档案键：`account:{服务器}:{用户名}`。
pub(crate) fn history_key_for_config(config: &SyncConfig) -> String {
    format!(
        "account:{}:{}",
        config.server_address.trim().to_ascii_lowercase(),
        config.username.trim().to_ascii_lowercase()
    )
}

/// 配置对应的服务器 HTTP 基址（`http(s)://{host}:{port}`）。校验规则与前端
/// `normalizeServerAddress`（apps/app/src/features/sync/syncSetup.ts）保持一致：
/// 只接受 `host:port`，不带 scheme 或路径。
pub(crate) fn server_http_url(config: &SyncConfig) -> Result<String, String> {
    let candidate = config.server_address.trim();
    if candidate.is_empty() {
        return Err("请输入服务器 IP 和端口".to_string());
    }
    if candidate.contains("://") || candidate.contains('/') {
        return Err("只需填写 IP 和端口，例如 192.168.1.20:4810".to_string());
    }
    let parsed = url::Url::parse(&format!("http://{candidate}"))
        .map_err(|_| "服务器地址格式不正确".to_string())?;
    let host_empty = parsed.host_str().map(str::is_empty).unwrap_or(true);
    if host_empty || parsed.port().is_none() {
        return Err("服务器地址必须包含 IP 和端口".to_string());
    }
    let secure = config.server_protocol == "https";
    Ok(format!(
        "{}://{}:{}",
        if secure { "https" } else { "http" },
        parsed.host_str().expect("checked above"),
        parsed.port().expect("checked above"),
    ))
}

pub(crate) fn load_sync_config(path: &Path) -> Option<SyncConfig> {
    let mut config: SyncConfig = fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())?;
    if config.session_token.is_empty() {
        // 文件已剥离 token：从系统凭据库补回；拿不到则视为未登录。
        config.session_token = token_store::load_session_token().unwrap_or_default();
    }
    Some(config)
}

pub(crate) fn write_sync_config(path: &Path, config: &Option<SyncConfig>) -> Result<(), String> {
    let mut persisted = config.clone();
    if let Some(config) = persisted.as_mut() {
        // token 优先进系统凭据库，成功则文件不落凭证；凭据库不可用时回落
        // 文件（行为与旧版一致）。空 token 表示登出：同样进 token_store
        // 清掉凭据库条目，避免重启后从凭据库「复活」已退出的会话。
        if token_store::store_session_token(&config.session_token) {
            config.session_token = String::new();
        }
    }
    let json = serde_json::to_string_pretty(&persisted).map_err(|error| error.to_string())?;
    // 原子写：直接覆盖的话，写一半崩溃/断电会留下无法解析的配置，下次启动
    // 回落到未登录档案——用户视角等于账号历史全部「消失」。
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
        let key = history
            .active_history
            .as_deref()
            .ok_or("同步账号未登录，历史档案不可用")?;
        preferences_path_for(&state.histories_dir, key)
    };
    write_preferences(&path, &preferences)?;
    *state
        .account_preferences
        .lock()
        .map_err(|error| error.to_string())? = preferences;
    Ok(())
}

/// 保存配置；键变化时切换到新档案的历史库。设备身份是机器级的
/// （`device.json`），不随档案切换。传 None（退出账号）即清空配置并停用
/// 档案：未登录时没有活动历史库，捕获与查询都不可用。
/// 顺序很讲究：先持久化旧档案元数据，再原子写配置文件，最后才切换内存
/// 档案。配置写失败时直接返回，内存档案原封不动——不会出现「内存已切到
/// 账号档案、磁盘配置还是旧档案」的分裂状态。
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn save_sync_config(
    state: State<'_, AppState>,
    config: Option<SyncConfig>,
) -> Result<(), String> {
    let history_key = config.as_ref().map(history_key_for_config);
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    if history.active_history != history_key {
        // Persist the outgoing profile's metadata before leaving it.
        if let Some(old_key) = history.active_history.clone() {
            let current_path = history_path_for_key(&state.histories_dir, &old_key);
            state.with_database(&current_path, |connection| save_metadata(connection, &history))?;
        }
    }
    write_sync_config(&state.sync_config_path, &config)?;
    if history.active_history != history_key {
        match &history_key {
            Some(key) => {
                let next_path = history_path_for_key(&state.histories_dir, key);
                *history = load_history(&next_path, key);
                // 偏好跟随档案：切到新档案后加载它的 preferences.json。
                let preferences_path = preferences_path_for(&state.histories_dir, key);
                let preferences = load_preferences(&preferences_path);
                *state
                    .account_preferences
                    .lock()
                    .map_err(|error| error.to_string())? = preferences;
            }
            None => {
                *history = HistoryData::default();
                *state
                    .account_preferences
                    .lock()
                    .map_err(|error| error.to_string())? = AccountPreferences::default();
            }
        }
    }
    if let Some(key) = history.active_history.clone() {
        let active_path = history_path_for_key(&state.histories_dir, &key);
        state.with_database(&active_path, |connection| save_metadata(connection, &history))?;
    }
    *state.sync_config.lock().map_err(|error| error.to_string())? = config;
    Ok(())
}
