//! 同步账号配置与它选定的历史档案。
//!
//! Main applies explicit connection changes; Rust notifies paste to reload the
//! authoritative session after persistence, avoiding a second main connection.

use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use tauri::{AppHandle, Emitter, Manager, State};

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
    /// 是否监听本机剪贴板（轮询捕获复制内容进历史）；关闭后仅接收同步内容。
    /// 桌面端监听线程每轮读取；旧 preferences.json 缺该字段时按 Default 取 true。
    pub watch_clipboard: bool,
    /// 捕获文件/文件夹时按名称跳过的过滤模式（`*`/`?` 通配，如 `node_modules`）。
    #[serde(default = "default_exclude_patterns")]
    pub exclude_patterns: Vec<String>,
    /// 登录时服务器下发的单文件存储上限（MB）；Rust 侧只透传持久化，前端用它
    /// 约束自动上传档位。
    #[serde(default = "default_server_max_file_mb")]
    pub server_max_file_mb: u64,
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
            watch_clipboard: default_watch_clipboard(),
            exclude_patterns: default_exclude_patterns(),
            server_max_file_mb: default_server_max_file_mb(),
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

fn default_watch_clipboard() -> bool {
    true
}

fn default_exclude_patterns() -> Vec<String> {
    vec!["node_modules".to_string()]
}

fn default_server_max_file_mb() -> u64 {
    200
}

fn default_max_capture_file_count() -> u64 {
    1000
}

/// 含已保存凭证的配置，用于缓存登录；不代表运行时已通过 login/verify。
/// 登出清空 token，保留服务器地址和用户名供登录页回填。
pub(crate) fn signed_in_config(config: &SyncConfig) -> Option<&SyncConfig> {
    (!config.session_token.trim().is_empty()).then_some(config)
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
    let history = state.history.lock().map_err(|error| error.to_string())?;
    if history.active_history.is_none() { return Err("同步账号未登录".to_string()); }
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
    session_id: String,
) -> Result<(), String> {
    let account = state.account(&session_id)?;
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let path = account.preferences_path.clone();
    write_preferences(&path, &preferences)?;
    // The runtime capture settings cache belongs only to the active archive.
    if history.active_history.as_deref() == Some(account.key.as_str()) {
        *state.account_preferences.lock().map_err(|error| error.to_string())? = preferences;
    }
    Ok(())
}

/// 保存登录配置不代表登录成功。停用旧账号后，等待 login/verify 的启用步骤。
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn save_sync_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: Option<SyncConfig>,
) -> Result<(), String> {
    write_sync_config(&state.sync_config_path, &config)?;
    state.deactivate_account()?;
    *state.sync_config.lock().map_err(|error| error.to_string())? = config;
    if let Some(paste) = app.get_webview_window("paste") { let _ = paste.hide(); }
    let _ = app.emit_to("paste", "cliproam://sync-config-changed", ());
    Ok(())
}
