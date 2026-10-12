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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum TokenStorage {
    Keyring,
    File,
    SignedOut,
}

/// 仅存在于磁盘；前端仍使用 SyncConfig。显式区分退出与钥匙串 token。
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSyncConfig {
    #[serde(flatten)]
    config: SyncConfig,
    #[serde(default)]
    token_storage: Option<TokenStorage>,
}

impl StoredSyncConfig {
    fn uses_keyring(&self) -> bool {
        self.token_storage == Some(TokenStorage::Keyring)
            || (self.token_storage.is_none() && self.config.session_token.is_empty())
    }

    fn resolve(
        mut self,
        load: impl FnOnce() -> Result<Option<String>, String>,
    ) -> Result<SyncConfig, String> {
        if self.uses_keyring() {
            self.config.session_token = load()?.unwrap_or_default();
        } else if self.token_storage == Some(TokenStorage::SignedOut) {
            self.config.session_token.clear();
        }
        Ok(self.config)
    }
}

fn read_stored_sync_config(path: &Path) -> Option<StoredSyncConfig> {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

pub(crate) fn load_sync_config(path: &Path) -> Option<SyncConfig> {
    // setup 在主线程，不在此处弹出或等待钥匙串 UI；交给异步读取命令。
    read_stored_sync_config(path).map(|stored| stored.config)
}

pub(crate) fn write_sync_config(path: &Path, config: &Option<SyncConfig>) -> Result<(), String> {
    write_sync_config_with(path, config, token_store::store_session_token)
}

fn write_sync_config_with(
    path: &Path,
    config: &Option<SyncConfig>,
    store: impl FnOnce(&str) -> Result<bool, String>,
) -> Result<(), String> {
    let signing_out = config
        .as_ref()
        .is_none_or(|config| config.session_token.is_empty());
    let stored_in_keyring = if signing_out {
        // 文件回落和已退出配置不依赖钥匙串，无凭据服务的平台仍能退出。
        if read_stored_sync_config(path).is_some_and(|previous| previous.uses_keyring()) {
            store("")?;
        }
        false
    } else {
        store(&config.as_ref().expect("signed-in config").session_token)?
    };
    let persisted = config.as_ref().map(|config| {
        let mut config = config.clone();
        let token_storage = if signing_out {
            TokenStorage::SignedOut
        } else if stored_in_keyring {
            config.session_token.clear();
            TokenStorage::Keyring
        } else {
            TokenStorage::File
        };
        StoredSyncConfig {
            config,
            token_storage: Some(token_storage),
        }
    });
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

#[tauri::command(async)]
pub(crate) fn get_sync_config(state: State<'_, AppState>) -> Result<Option<SyncConfig>, String> {
    let _io = state
        .sync_config_io
        .lock()
        .map_err(|error| error.to_string())?;
    let current = state
        .sync_config
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    let Some(config) = current
        .as_ref()
        .filter(|config| config.session_token.is_empty())
    else {
        return Ok(current);
    };
    let Some(stored) = read_stored_sync_config(&state.sync_config_path) else {
        return Ok(current);
    };
    if stored.config != *config || !stored.uses_keyring() {
        return Ok(current);
    }
    // 每次重试重新访问凭据库，不缓存授权拒绝；等待系统 UI 时不持状态锁。
    let recovered = stored.resolve(token_store::load_session_token)?;
    let mut latest = state
        .sync_config
        .lock()
        .map_err(|error| error.to_string())?;
    if *latest == current {
        *latest = Some(recovered);
    }
    Ok(latest.clone())
}

#[tauri::command]
pub(crate) fn get_account_preferences(
    state: State<'_, AppState>,
) -> Result<AccountPreferences, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    if history.active_history.is_none() {
        return Err("同步账号未登录".to_string());
    }
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
        *state
            .account_preferences
            .lock()
            .map_err(|error| error.to_string())? = preferences;
    }
    Ok(())
}

/// 保存登录配置不代表登录成功。停用旧账号后，等待 login/verify 的启用步骤。
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn save_sync_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: Option<SyncConfig>,
) -> Result<(), String> {
    let _io = state
        .sync_config_io
        .lock()
        .map_err(|error| error.to_string())?;
    write_sync_config(&state.sync_config_path, &config)?;
    state.deactivate_account()?;
    *state
        .sync_config
        .lock()
        .map_err(|error| error.to_string())? = config;
    if let Some(paste) = app.get_webview_window("paste") {
        let _ = paste.hide();
    }
    let _ = app.emit_to("paste", "cliproam://sync-config-changed", ());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct ConfigFile(PathBuf);

    impl ConfigFile {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("cliproam-config-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&directory).unwrap();
            Self(directory.join("sync-config.json"))
        }
    }

    impl Drop for ConfigFile {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    fn config(token: &str) -> SyncConfig {
        SyncConfig {
            server_address: "localhost:4810".into(),
            server_protocol: "http".into(),
            username: "user".into(),
            session_token: token.into(),
        }
    }

    #[test]
    fn failed_logout_keeps_saved_session_and_successful_logout_never_restores_it() {
        let file = ConfigFile::new();
        write_sync_config_with(&file.0, &Some(config("saved-token")), |_| Ok(true)).unwrap();
        let before = fs::read(&file.0).unwrap();
        let error = write_sync_config_with(&file.0, &Some(config("")), |token| {
            assert!(token.is_empty());
            Err("keychain access denied".into())
        });
        assert!(error.is_err());
        assert_eq!(fs::read(&file.0).unwrap(), before);

        write_sync_config_with(&file.0, &Some(config("")), |token| {
            assert!(token.is_empty());
            Ok(true)
        })
        .unwrap();
        let stored = read_stored_sync_config(&file.0).unwrap();
        assert!(stored.token_storage == Some(TokenStorage::SignedOut));
        let signed_out = stored
            .resolve(|| panic!("logout must not consult old keychain credentials"))
            .unwrap();
        assert!(signed_out.session_token.is_empty());
        assert_eq!(signed_out.username, "user");
    }

    #[test]
    fn saved_keychain_session_can_be_reloaded_after_access_is_granted() {
        let file = ConfigFile::new();
        write_sync_config_with(&file.0, &Some(config("saved-token")), |_| Ok(true)).unwrap();
        assert!(!fs::read_to_string(&file.0).unwrap().contains("saved-token"));
        assert!(load_sync_config(&file.0).unwrap().session_token.is_empty());
        assert!(read_stored_sync_config(&file.0)
            .unwrap()
            .resolve(|| Err("access denied".into()))
            .is_err());
        let restored = read_stored_sync_config(&file.0)
            .unwrap()
            .resolve(|| Ok(Some("saved-token".into())))
            .unwrap();
        assert_eq!(restored.session_token, "saved-token");
    }

    #[test]
    fn file_fallback_and_logout_do_not_require_an_available_keychain() {
        let file = ConfigFile::new();
        write_sync_config_with(&file.0, &Some(config("file-token")), |_| Ok(false)).unwrap();
        let restored = read_stored_sync_config(&file.0)
            .unwrap()
            .resolve(|| panic!("file token must not read an older keychain token"))
            .unwrap();
        assert_eq!(restored.session_token, "file-token");
        write_sync_config_with(&file.0, &Some(config("")), |_| {
            panic!("file fallback logout must work without keychain access")
        })
        .unwrap();
        assert!(read_stored_sync_config(&file.0)
            .unwrap()
            .resolve(|| panic!("signed out"))
            .unwrap()
            .session_token
            .is_empty());
    }

    #[test]
    fn clearing_config_deletes_saved_keychain_credentials() {
        let file = ConfigFile::new();
        write_sync_config_with(&file.0, &Some(config("saved-token")), |_| Ok(true)).unwrap();
        let mut deleted = false;
        write_sync_config_with(&file.0, &None, |token| {
            assert!(token.is_empty());
            deleted = true;
            Ok(true)
        })
        .unwrap();
        assert!(deleted);
        assert!(load_sync_config(&file.0).is_none());
    }

    #[test]
    fn legacy_config_without_storage_marker_remains_readable() {
        let legacy = serde_json::to_string(&config("")).unwrap();
        let stored: StoredSyncConfig = serde_json::from_str(&legacy).unwrap();
        assert_eq!(
            stored
                .resolve(|| Ok(Some("legacy-keychain-token".into())))
                .unwrap()
                .session_token,
            "legacy-keychain-token"
        );
        let legacy = serde_json::to_string(&config("legacy-file-token")).unwrap();
        let stored: StoredSyncConfig = serde_json::from_str(&legacy).unwrap();
        assert_eq!(
            stored
                .resolve(|| panic!("legacy file fallback"))
                .unwrap()
                .session_token,
            "legacy-file-token"
        );
    }
}
