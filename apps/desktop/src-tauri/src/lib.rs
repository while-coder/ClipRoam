use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use image::{GenericImageView, ImageFormat};
use rusqlite::{params, params_from_iter, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::{Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    thread,
    time::{Duration, UNIX_EPOCH},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, State,
};
#[cfg(not(target_os = "windows"))]
use tauri_plugin_clipboard_manager::ClipboardExt;
use uuid::Uuid;

const MAX_UNPINNED_ENTRIES: usize = 200;
const LOCAL_HISTORY_KEY: &str = "local";
const TRAY_SHOW_MAIN: &str = "show-main";
const TRAY_QUIT: &str = "quit";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClipboardFile {
    id: String,
    name: String,
    size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
    location: String,
    available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local_modified_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClipboardEntry {
    id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    client_id: String,
    kind: String,
    content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    html: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rtf: Option<String>,
    #[serde(default)]
    files: Vec<ClipboardFile>,
    source_device_id: String,
    created_at: String,
    #[serde(default)]
    pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncConfig {
    enabled: bool,
    #[serde(default, alias = "serverUrl")]
    server_address: String,
    #[serde(default = "default_server_protocol")]
    server_protocol: String,
    #[serde(default)]
    username: String,
    #[serde(default, alias = "token")]
    session_token: String,
    #[serde(default = "default_auto_upload_limit_mb")]
    auto_upload_limit_mb: u64,
}

fn default_server_protocol() -> String {
    "http".to_string()
}

fn default_auto_upload_limit_mb() -> u64 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryData {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    entries: Vec<ClipboardEntry>,
    #[serde(default)]
    histories: HashMap<String, Vec<ClipboardEntry>>,
    #[serde(default = "default_active_history")]
    active_history: String,
    #[serde(default)]
    last_clipboard: String,
    #[serde(default)]
    last_file_signature: String,
    #[serde(default)]
    last_image_signature: String,
    device_id: String,
    device_name: String,
}

impl Default for HistoryData {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            histories: HashMap::new(),
            active_history: default_active_history(),
            last_clipboard: String::new(),
            last_file_signature: String::new(),
            last_image_signature: String::new(),
            device_id: Uuid::new_v4().to_string(),
            device_name: std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "This device".to_string()),
        }
    }
}

impl HistoryData {
    fn active_entries(&self) -> &[ClipboardEntry] {
        self.histories
            .get(&self.active_history)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn active_entries_mut(&mut self) -> &mut Vec<ClipboardEntry> {
        self.histories
            .entry(self.active_history.clone())
            .or_default()
    }
}

struct AppState {
    history: Mutex<HistoryData>,
    histories_dir: PathBuf,
    sync_config: Mutex<Option<SyncConfig>>,
    sync_config_path: PathBuf,
    downloads: Mutex<HashMap<String, DownloadState>>,
}

struct DownloadState {
    path: PathBuf,
    expected_size: u64,
    received_size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MissingFile {
    id: String,
    name: String,
    size: u64,
    source_device_id: String,
}

#[derive(Debug, Clone)]
struct RichText {
    text: String,
    html: Option<String>,
    rtf: Option<String>,
}

fn default_active_history() -> String {
    LOCAL_HISTORY_KEY.to_string()
}

fn history_key_for_config(config: &SyncConfig) -> String {
    if config.enabled && !config.username.trim().is_empty() {
        format!(
            "account:{}:{}",
            config.server_address.trim().to_ascii_lowercase(),
            config.username.trim().to_ascii_lowercase()
        )
    } else {
        default_active_history()
    }
}

fn history_path_for_key(histories_dir: &Path, key: &str) -> PathBuf {
    histories_dir
        .join(format!("{}-{:016x}", safe_history_directory_name(key), stable_key_hash(key)))
        .join("history.sqlite")
}

fn safe_history_directory_name(key: &str) -> String {
    let name = key
        .strip_prefix("account:")
        .unwrap_or(key)
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if name.is_empty() { "local".to_string() } else { name }
}

fn stable_key_hash(key: &str) -> u64 {
    key.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

fn open_history_database(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS entries (
                id TEXT PRIMARY KEY,
                client_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL,
                pinned INTEGER NOT NULL,
                source_device_id TEXT NOT NULL,
                source_app TEXT NOT NULL DEFAULT '',
                payload_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS entries_created_at ON entries(created_at DESC);
            CREATE INDEX IF NOT EXISTS entries_kind_created_at ON entries(kind, created_at DESC);
            CREATE INDEX IF NOT EXISTS entries_source_app_created_at ON entries(source_app, created_at DESC);
            CREATE TABLE IF NOT EXISTS tags (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE COLLATE NOCASE
            );
            CREATE TABLE IF NOT EXISTS entry_tags (
                entry_id TEXT NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
                tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                PRIMARY KEY (entry_id, tag_id)
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
                entry_id UNINDEXED,
                content,
                file_names,
                pinyin
            );
            ",
        )
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

fn load_history(path: &Path, key: &str) -> HistoryData {
    let mut history = HistoryData {
        active_history: key.to_string(),
        ..HistoryData::default()
    };
    let Ok(connection) = open_history_database(path) else {
        history.histories.insert(key.to_string(), Vec::new());
        return history;
    };

    if let Ok(mut statement) = connection.prepare("SELECT key, value FROM metadata") {
        if let Ok(rows) = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))) {
            for row in rows.flatten() {
                match row.0.as_str() {
                    "last_clipboard" => history.last_clipboard = row.1,
                    "last_file_signature" => history.last_file_signature = row.1,
                    "last_image_signature" => history.last_image_signature = row.1,
                    "device_id" => history.device_id = row.1,
                    "device_name" => history.device_name = row.1,
                    _ => {}
                }
            }
        }
    }

    let mut entries = Vec::new();
    if let Ok(mut statement) = connection.prepare("SELECT payload_json FROM entries ORDER BY pinned DESC, created_at DESC") {
        if let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(0)) {
            for raw in rows.flatten() {
                if let Ok(mut entry) = serde_json::from_str::<ClipboardEntry>(&raw) {
                    if entry.client_id.is_empty() {
                        entry.client_id = Uuid::parse_str(&entry.id)
                            .map(|id| id.to_string())
                            .unwrap_or_else(|_| Uuid::new_v4().to_string());
                    }
                    entries.push(entry);
                }
            }
        }
    }
    history.histories.insert(key.to_string(), entries);
    history
}

fn save_history(path: &Path, history: &HistoryData) -> Result<(), String> {
    let mut connection = open_history_database(path)?;
    let transaction = connection.transaction().map_err(|error| error.to_string())?;
    transaction.execute("DELETE FROM entries_fts", []).map_err(|error| error.to_string())?;
    let entry_ids = history.active_entries().iter().map(|entry| entry.id.as_str()).collect::<Vec<_>>();
    if entry_ids.is_empty() {
        transaction.execute("DELETE FROM entries", []).map_err(|error| error.to_string())?;
    } else {
        let placeholders = std::iter::repeat("?").take(entry_ids.len()).collect::<Vec<_>>().join(", ");
        transaction
            .execute(
                &format!("DELETE FROM entries WHERE id NOT IN ({placeholders})"),
                params_from_iter(entry_ids),
            )
            .map_err(|error| error.to_string())?;
    }
    for entry in history.active_entries() {
        let payload_json = serde_json::to_string(entry).map_err(|error| error.to_string())?;
        let file_names = entry.files.iter().map(|file| file.name.as_str()).collect::<Vec<_>>().join("\n");
        transaction
            .execute(
                "INSERT INTO entries (id, client_id, kind, content, created_at, pinned, source_device_id, payload_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET client_id = excluded.client_id, kind = excluded.kind, content = excluded.content, created_at = excluded.created_at, pinned = excluded.pinned, source_device_id = excluded.source_device_id, payload_json = excluded.payload_json",
                params![
                    entry.id,
                    entry.client_id,
                    entry.kind,
                    entry.content,
                    entry.created_at,
                    entry.pinned,
                    entry.source_device_id,
                    payload_json,
                ],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO entries_fts (entry_id, content, file_names, pinyin) VALUES (?, ?, ?, '')",
                params![entry.id, entry.content, file_names],
            )
            .map_err(|error| error.to_string())?;
    }
    for (key, value) in [
        ("last_clipboard", history.last_clipboard.as_str()),
        ("last_file_signature", history.last_file_signature.as_str()),
        ("last_image_signature", history.last_image_signature.as_str()),
        ("device_id", history.device_id.as_str()),
        ("device_name", history.device_name.as_str()),
    ] {
        transaction
            .execute(
                "INSERT INTO metadata (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())
}

fn save_active_history(state: &AppState, history: &HistoryData) -> Result<(), String> {
    save_history(
        &history_path_for_key(&state.histories_dir, &history.active_history),
        history,
    )
}

fn active_cache_dir(state: &AppState, history: &HistoryData) -> PathBuf {
    history_path_for_key(&state.histories_dir, &history.active_history)
        .parent()
        .expect("history file always has a parent directory")
        .join("files")
}

fn is_cached_image_path(state: &AppState, path: &Path) -> bool {
    path.starts_with(&state.histories_dir)
        && path.parent().and_then(Path::file_name).is_some_and(|name| name == "images")
        && path.parent().and_then(Path::parent).and_then(Path::file_name).is_some_and(|name| name == "files")
}

fn load_sync_config(path: &Path) -> Option<SyncConfig> {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

fn write_sync_config(path: &Path, config: &Option<SyncConfig>) -> Result<(), String> {
    let json = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

fn retain_single_history(history: &mut HistoryData, key: &str) {
    let entries = history.histories.remove(key).unwrap_or_default();
    history.entries.clear();
    history.histories.clear();
    history.histories.insert(key.to_string(), entries);
    history.active_history = key.to_string();
}

fn trim_history(entries: &mut Vec<ClipboardEntry>) {
    let mut unpinned = 0usize;
    entries.retain(|entry| {
        if entry.pinned {
            true
        } else {
            unpinned += 1;
            unpinned <= MAX_UNPINNED_ENTRIES
        }
    });
}

fn rich_text_signature(rich_text: &RichText) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for value in [
        Some(rich_text.text.as_str()),
        rich_text.html.as_deref(),
        rich_text.rtf.as_deref(),
    ] {
        for byte in value.unwrap_or_default().bytes().chain(std::iter::once(0)) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    format!("{hash:016x}")
}

fn capture_text(app: &AppHandle, rich_text: RichText) -> Result<(), String> {
    if rich_text.text.trim().is_empty() {
        return Ok(());
    }
    let signature = rich_text_signature(&rich_text);
    let state = app.state::<AppState>();
    let entry = {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        if history.last_clipboard == signature {
            return Ok(());
        }
        history.last_clipboard = signature;
        history.last_file_signature.clear();
        history.last_image_signature.clear();
        let device_id = history.device_id.clone();
        let client_id = Uuid::new_v4().to_string();
        let entry = ClipboardEntry {
            id: client_id.clone(),
            client_id,
            kind: "text".to_string(),
            content: rich_text.text,
            html: rich_text.html,
            rtf: rich_text.rtf,
            files: Vec::new(),
            source_device_id: device_id,
            created_at: Utc::now().to_rfc3339(),
            pinned: false,
        };
        let entries = history.active_entries_mut();
        entries.retain(|item| item.content != entry.content);
        entries.insert(0, entry.clone());
        trim_history(entries);
        save_active_history(&state, &history)?;
        entry
    };
    app.emit("cliproam://entry-created", entry)
        .map_err(|error| error.to_string())
}

fn capture_files(app: &AppHandle, paths: Vec<PathBuf>) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let signature = file_signature(&paths);
    let state = app.state::<AppState>();
    let entry = {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        if history.last_file_signature == signature {
            return Ok(());
        }
        history.last_file_signature = signature.clone();
        history.last_clipboard.clear();
        history.last_image_signature.clear();
        let device_id = history.device_id.clone();
        let files = paths
            .iter()
            .map(|path| {
                let metadata = path.metadata().ok();
                ClipboardFile {
                    id: Uuid::new_v4().to_string(),
                    name: path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string()),
                    size: metadata.as_ref().map(|value| value.len()).unwrap_or(0),
                    mime: None,
                    sha256: None,
                    location: "device".to_string(),
                    available: path.exists(),
                    local_path: Some(path.display().to_string()),
                    local_modified_at: metadata
                        .and_then(|value| value.modified().ok())
                        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                        .map(|value| value.as_millis() as u64),
                }
            })
            .collect::<Vec<_>>();
        let content = if files.len() == 1 {
            files[0].name.clone()
        } else {
            format!("{} 等 {} 个文件", files[0].name, files.len())
        };
        let client_id = Uuid::new_v4().to_string();
        let entry = ClipboardEntry {
            id: client_id.clone(),
            client_id,
            kind: "files".to_string(),
            content,
            html: None,
            rtf: None,
            files,
            source_device_id: device_id,
            created_at: Utc::now().to_rfc3339(),
            pinned: false,
        };
        let entries = history.active_entries_mut();
        let entry = if let Some(index) = entries
            .iter()
            .position(|item| item.kind == "files" && file_entry_signature(item) == signature)
        {
            let mut existing = entries.remove(index);
            existing.created_at = entry.created_at;
            entries.insert(0, existing.clone());
            existing
        } else {
            entries.insert(0, entry.clone());
            entry
        };
        trim_history(entries);
        save_active_history(&state, &history)?;
        entry
    };
    app.emit("cliproam://entry-created", entry)
        .map_err(|error| error.to_string())
}

fn file_signature(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| {
            let metadata = path.metadata().ok();
            let size = metadata.as_ref().map(|value| value.len()).unwrap_or(0);
            let modified_at = metadata
                .and_then(|value| value.modified().ok())
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_millis())
                .unwrap_or_default();
            format!("{}:{size}:{modified_at}", path.to_string_lossy().to_ascii_lowercase())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn file_entry_signature(entry: &ClipboardEntry) -> String {
    entry
        .files
        .iter()
        .filter_map(|file| file.local_path.as_deref().map(|path| {
            format!(
                "{}:{}:{}",
                path.to_ascii_lowercase(),
                file.size,
                file.local_modified_at.unwrap_or_default(),
            )
        }))
        .collect::<Vec<_>>()
        .join("\n")
}

fn preserve_local_file_paths(remote: &mut ClipboardEntry, local: &ClipboardEntry) {
    for (index, remote_file) in remote.files.iter_mut().enumerate() {
        let local_file = local.files.iter().find(|file| file.id == remote_file.id).or_else(|| {
            local.files.get(index).filter(|file| file.name == remote_file.name && file.size == remote_file.size)
        });
        let Some(local_file) = local_file else {
            continue;
        };
        if let Some(path) = &local_file.local_path {
            remote_file.local_path = Some(path.clone());
            remote_file.available = Path::new(path).exists();
            remote_file.local_modified_at = local_file.local_modified_at;
        }
    }
}

#[cfg(target_os = "windows")]
fn read_clipboard_files() -> Option<Vec<PathBuf>> {
    clipboard_win::get_clipboard(clipboard_win::formats::FileList).ok()
}

#[cfg(not(target_os = "windows"))]
fn read_clipboard_files() -> Option<Vec<PathBuf>> {
    None
}

#[cfg(target_os = "windows")]
fn read_clipboard_image() -> Option<Vec<u8>> {
    use clipboard_win::{formats::Bitmap, Clipboard, Getter};

    let _clipboard = Clipboard::new_attempts(10).ok()?;
    let mut image = Vec::new();
    Bitmap.read_clipboard(&mut image).ok()?;
    (!image.is_empty()).then_some(image)
}

#[cfg(not(target_os = "windows"))]
fn read_clipboard_image() -> Option<Vec<u8>> {
    None
}

#[cfg(target_os = "windows")]
fn read_clipboard_text(_app: &AppHandle) -> Option<RichText> {
    use clipboard_win::{
        formats::{Html, RawData, Unicode},
        raw, Clipboard, Getter,
    };

    let _clipboard = Clipboard::new_attempts(10).ok()?;
    let mut text = String::new();
    Unicode.read_clipboard(&mut text).ok()?;
    if text.trim().is_empty() {
        return None;
    }

    let html = Html::new().and_then(|format| {
        let mut value = String::new();
        format
            .read_clipboard(&mut value)
            .ok()
            .filter(|_| !value.is_empty())
            .map(|_| value)
    });
    let rtf = raw::register_format("Rich Text Format").and_then(|format| {
        let mut value = Vec::new();
        RawData(format.get())
            .read_clipboard(&mut value)
            .ok()
            .and_then(|_| String::from_utf8(value).ok())
            .map(|value| value.trim_end_matches('\0').to_string())
            .filter(|value| !value.is_empty())
    });

    Some(RichText { text, html, rtf })
}

#[cfg(not(target_os = "windows"))]
fn read_clipboard_text(app: &AppHandle) -> Option<RichText> {
    app.clipboard().read_text().ok().and_then(|text| {
        (!text.trim().is_empty()).then_some(RichText {
            text,
            html: None,
            rtf: None,
        })
    })
}

fn image_signature(image: &[u8]) -> String {
    // FNV-1a is sufficient here: this only suppresses repeated reads of the current clipboard.
    let hash = image.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    format!("{}:{hash:016x}", image.len())
}

fn encode_image_as_webp(image: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let decoded = image::load_from_memory_with_format(image, ImageFormat::Bmp)
        .map_err(|error| error.to_string())?;
    let (width, height) = decoded.dimensions();
    let mut output = Cursor::new(Vec::new());
    decoded
        .write_to(&mut output, ImageFormat::WebP)
        .map_err(|error| error.to_string())?;
    Ok((output.into_inner(), width, height))
}

fn decode_image_as_bmp(image: &[u8]) -> Result<Vec<u8>, String> {
    let decoded = image::load_from_memory(image).map_err(|error| error.to_string())?;
    let mut output = Cursor::new(Vec::new());
    decoded
        .write_to(&mut output, ImageFormat::Bmp)
        .map_err(|error| error.to_string())?;
    Ok(output.into_inner())
}

fn capture_image(app: &AppHandle, image: Vec<u8>) -> Result<(), String> {
    let signature = image_signature(&image);
    let (webp, width, height) = encode_image_as_webp(&image)?;
    let state = app.state::<AppState>();
    let entry = {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        if history.last_image_signature == signature {
            return Ok(());
        }

        let id = Uuid::new_v4().to_string();
        let image_dir = active_cache_dir(&state, &history).join("images");
        fs::create_dir_all(&image_dir).map_err(|error| error.to_string())?;
        let image_path = image_dir.join(format!("{id}.webp"));
        fs::write(&image_path, &webp).map_err(|error| error.to_string())?;

        history.last_image_signature = signature;
        history.last_clipboard.clear();
        history.last_file_signature.clear();
        let device_id = history.device_id.clone();
        let description = format!("截图（{width} × {height}）");
        let entry = ClipboardEntry {
            id: id.clone(),
            client_id: id.clone(),
            kind: "image".to_string(),
            content: description,
            html: None,
            rtf: None,
            files: vec![ClipboardFile {
                id: Uuid::new_v4().to_string(),
                name: format!("{id}.webp"),
                size: webp.len() as u64,
                mime: Some("image/webp".to_string()),
                sha256: None,
                location: "device".to_string(),
                available: true,
                local_path: Some(image_path.display().to_string()),
                local_modified_at: None,
            }],
            source_device_id: device_id,
            created_at: Utc::now().to_rfc3339(),
            pinned: false,
        };
        history.active_entries_mut().insert(0, entry.clone());
        trim_history(history.active_entries_mut());
        save_active_history(&state, &history)?;
        entry
    };
    app.emit("cliproam://entry-created", entry)
        .map_err(|error| error.to_string())
}

fn start_clipboard_monitor(app: AppHandle) {
    thread::spawn(move || loop {
        if let Some(paths) = read_clipboard_files().filter(|paths| !paths.is_empty()) {
            let _ = capture_files(&app, paths);
        } else if let Some(rich_text) = read_clipboard_text(&app) {
            let _ = capture_text(&app, rich_text);
        } else if let Some(image) = read_clipboard_image() {
            let _ = capture_image(&app, image);
        }
        thread::sleep(Duration::from_millis(350));
    });
}

#[tauri::command]
fn list_entries(state: State<'_, AppState>) -> Result<Vec<ClipboardEntry>, String> {
    Ok(state
        .history
        .lock()
        .map_err(|error| error.to_string())?
        .active_entries()
        .to_vec())
}

#[tauri::command]
fn get_device(state: State<'_, AppState>) -> Result<(String, String), String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    Ok((history.device_id.clone(), history.device_name.clone()))
}

#[tauri::command(rename_all = "camelCase")]
fn configure_device(
    state: State<'_, AppState>,
    device_id: String,
    device_name: String,
) -> Result<(), String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    history.device_id = device_id;
    history.device_name = device_name;
    save_active_history(&state, &history)
}

#[tauri::command]
fn get_sync_config(state: State<'_, AppState>) -> Result<Option<SyncConfig>, String> {
    Ok(state
        .sync_config
        .lock()
        .map_err(|error| error.to_string())?
        .clone())
}

#[tauri::command]
fn open_app_data_dir(app: AppHandle) -> Result<(), String> {
    let app_data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    fs::create_dir_all(&app_data_dir).map_err(|error| error.to_string())?;

    #[cfg(target_os = "windows")]
    Command::new("explorer.exe")
        .arg(&app_data_dir)
        .spawn()
        .map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    Command::new("open")
        .arg(&app_data_dir)
        .spawn()
        .map_err(|error| error.to_string())?;
    #[cfg(all(unix, not(target_os = "macos")))]
    Command::new("xdg-open")
        .arg(&app_data_dir)
        .spawn()
        .map_err(|error| error.to_string())?;

    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
fn save_sync_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: SyncConfig,
) -> Result<(), String> {
    let history_key = history_key_for_config(&config);
    {
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
    }
    let config = Some(config);
    write_sync_config(&state.sync_config_path, &config)?;
    *state.sync_config.lock().map_err(|error| error.to_string())? = config;
    app.emit("cliproam://sync-config-changed", ())
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn upsert_remote_entry(
    app: AppHandle,
    state: State<'_, AppState>,
    mut entry: ClipboardEntry,
) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let entries = history.active_entries_mut();
        if let Some(local) = entries.iter().find(|item| {
            item.id == entry.id || (!entry.client_id.is_empty() && item.client_id == entry.client_id)
        }) {
            preserve_local_file_paths(&mut entry, local);
            if entry.id == entry.client_id && local.id != local.client_id {
                entry.id = local.id.clone();
            }
        }
        entries.retain(|item| {
            item.id != entry.id && (entry.client_id.is_empty() || item.client_id != entry.client_id)
        });
        entries.push(entry);
        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        trim_history(entries);
        save_active_history(&state, &history)?;
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn set_pinned(
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
    pinned: bool,
) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        if let Some(entry) = history
            .active_entries_mut()
            .iter_mut()
            .find(|entry| entry.id == entry_id || entry.client_id == entry_id)
        {
            entry.pinned = pinned;
        }
        save_active_history(&state, &history)?;
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn delete_entry(
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<(), String> {
    let removed_files = {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let entries = history.active_entries_mut();
        let removed_files = entries
            .iter()
            .filter(|entry| entry.id == entry_id && entry.kind == "image")
            .flat_map(|entry| entry.files.iter())
            .filter_map(|file| file.local_path.clone())
            .collect::<Vec<_>>();
        entries.retain(|entry| entry.id != entry_id);
        save_active_history(&state, &history)?;
        removed_files
    };
    for path in removed_files {
        let path = PathBuf::from(path);
        if is_cached_image_path(&state, &path) {
            let _ = fs::remove_file(path);
        }
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn clear_history(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let removed_files = {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let entries = history.active_entries_mut();
        let removed_files = entries
            .iter()
            .filter(|entry| !entry.pinned && entry.kind == "image")
            .flat_map(|entry| entry.files.iter())
            .filter_map(|file| file.local_path.clone())
            .collect::<Vec<_>>();
        entries.retain(|entry| entry.pinned);
        save_active_history(&state, &history)?;
        removed_files
    };
    for path in removed_files {
        let path = PathBuf::from(path);
        if is_cached_image_path(&state, &path) {
            let _ = fs::remove_file(path);
        }
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn open_paste(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("paste")
        .ok_or_else(|| "paste window is unavailable".to_string())?;
    position_history_window(&window)?;
    window.show().map_err(|error| error.to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    window
        .emit("cliproam://focus-search", ())
        .map_err(|error| error.to_string())
}

fn show_main_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let show_main = MenuItem::with_id(app, TRAY_SHOW_MAIN, "显示主界面", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, TRAY_QUIT, "退出 ClipRoam", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_main, &quit])?;
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or("application icon is unavailable")?;

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("ClipRoam")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            TRAY_SHOW_MAIN => {
                let _ = show_main_window(app);
            }
            TRAY_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                let _ = show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn position_history_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    let cursor = window
        .cursor_position()
        .map_err(|error| error.to_string())?;
    let Some(monitor) = window
        .monitor_from_point(cursor.x, cursor.y)
        .map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    let window_size = window.outer_size().map_err(|error| error.to_string())?;
    let work_area = monitor.work_area();
    let position = calculate_history_position(
        cursor.x.round() as i32,
        cursor.y.round() as i32,
        work_area.position.x,
        work_area.position.y,
        work_area.size.width,
        work_area.size.height,
        window_size.width,
        window_size.height,
    );

    window
        .set_position(position)
        .map_err(|error| error.to_string())
}

fn calculate_history_position(
    cursor_x: i32,
    cursor_y: i32,
    work_x: i32,
    work_y: i32,
    work_width: u32,
    work_height: u32,
    window_width: u32,
    window_height: u32,
) -> PhysicalPosition<i32> {
    const CURSOR_GAP: i32 = 12;
    const SCREEN_MARGIN: i32 = 8;
    let width = window_width as i32;
    let height = window_height as i32;
    let min_x = work_x + SCREEN_MARGIN;
    let min_y = work_y + SCREEN_MARGIN;
    let max_x = (work_x + work_width as i32 - width - SCREEN_MARGIN).max(min_x);
    let max_y = (work_y + work_height as i32 - height - SCREEN_MARGIN).max(min_y);
    let x = (cursor_x - width / 2).clamp(min_x, max_x);
    let below_cursor = cursor_y + CURSOR_GAP;
    let preferred_y = if below_cursor <= max_y {
        below_cursor
    } else {
        cursor_y - height - CURSOR_GAP
    };

    PhysicalPosition::new(x, preferred_y.clamp(min_y, max_y))
}

#[tauri::command]
fn hide_paste(app: AppHandle) -> Result<(), String> {
    app.get_webview_window("paste")
        .ok_or_else(|| "paste window is unavailable".to_string())?
        .hide()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn hide_main(app: AppHandle) -> Result<(), String> {
    app.get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?
        .hide()
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn synthesize_paste() -> Result<(), String> {
    use std::mem::size_of;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL, VK_V,
    };
    fn key(vk: u16, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }
    let inputs = [
        key(VK_CONTROL, 0),
        key(VK_V, 0),
        key(VK_V, KEYEVENTF_KEYUP),
        key(VK_CONTROL, KEYEVENTF_KEYUP),
    ];
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent == inputs.len() as u32 {
        Ok(())
    } else {
        Err(format!(
            "SendInput inserted {sent} of {} events",
            inputs.len()
        ))
    }
}

#[cfg(not(target_os = "windows"))]
fn synthesize_paste() -> Result<(), String> {
    Ok(())
}

enum ClipboardPayload {
    Text(RichText),
    Files(Vec<String>),
    Image(Vec<u8>),
}

#[cfg(target_os = "windows")]
fn write_clipboard_files(paths: &[String]) -> Result<(), String> {
    use clipboard_win::{formats::FileList, Clipboard, Setter};

    let _clipboard = Clipboard::new_attempts(10).map_err(|error| error.to_string())?;
    FileList
        .write_clipboard(paths)
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn write_clipboard_image(image: &[u8]) -> Result<(), String> {
    use clipboard_win::{formats::Bitmap, Clipboard, Setter};

    let bitmap = decode_image_as_bmp(image)?;
    let _clipboard = Clipboard::new_attempts(10).map_err(|error| error.to_string())?;
    Bitmap
        .write_clipboard(&bitmap)
        .map_err(|error| error.to_string())
}

#[cfg(not(target_os = "windows"))]
fn write_clipboard_image(_image: &[u8]) -> Result<(), String> {
    Err("当前平台暂不支持图片粘贴".to_string())
}

fn write_clipboard_text(_app: &AppHandle, rich_text: &RichText) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use clipboard_win::{
            formats::{Html, Unicode},
            raw, Clipboard, Setter,
        };

        let _clipboard = Clipboard::new_attempts(10).map_err(|error| error.to_string())?;
        Unicode
            .write_clipboard(&rich_text.text)
            .map_err(|error| error.to_string())?;
        if let Some(html) = &rich_text.html {
            if let Some(format) = Html::new() {
                format
                    .write_clipboard(html)
                    .map_err(|error| error.to_string())?;
            }
        }
        if let Some(rtf) = &rich_text.rtf {
            let format = raw::register_format("Rich Text Format")
                .ok_or_else(|| "无法注册 RTF 剪贴板格式".to_string())?;
            let mut rtf = rtf.clone().into_bytes();
            rtf.push(0);
            raw::set_without_clear(format.get(), &rtf).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        _app.clipboard()
            .write_text(&rich_text.text)
            .map_err(|error| error.to_string())
    }
}

#[cfg(not(target_os = "windows"))]
fn write_clipboard_files(_paths: &[String]) -> Result<(), String> {
    Err("当前平台暂不支持文件粘贴".to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn get_missing_files(
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<Vec<MissingFile>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let entry = history
        .active_entries()
        .iter()
        .find(|entry| entry.id == entry_id)
        .ok_or_else(|| "clipboard entry was not found".to_string())?;
    Ok(entry
        .files
        .iter()
        .filter(|file| {
            file.local_path
                .as_deref()
                .is_none_or(|path| !Path::new(path).exists())
        })
        .map(|file| MissingFile {
            id: file.id.clone(),
            name: file.name.clone(),
            size: file.size,
            source_device_id: entry.source_device_id.clone(),
        })
        .collect())
}

#[tauri::command(rename_all = "camelCase")]
fn read_file_chunk(
    state: State<'_, AppState>,
    entry_id: String,
    file_id: String,
    offset: u64,
    length: usize,
) -> Result<String, String> {
    let path = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        history
            .active_entries()
            .iter()
            .find(|entry| entry.id == entry_id || entry.client_id == entry_id)
            .and_then(|entry| entry.files.iter().find(|file| file.id == file_id))
            .and_then(|file| file.local_path.clone())
            .ok_or_else(|| "本机文件路径不可用".to_string())?
    };
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| error.to_string())?;
    let mut bytes = vec![0; length.min(128 * 1024)];
    let count = file.read(&mut bytes).map_err(|error| error.to_string())?;
    bytes.truncate(count);
    Ok(BASE64.encode(bytes))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileUploadSource {
    full_path: String,
    size: u64,
    modified_at: u64,
}

#[tauri::command(rename_all = "camelCase")]
fn get_file_upload_source(
    state: State<'_, AppState>,
    entry_id: String,
    file_id: String,
) -> Result<FileUploadSource, String> {
    let path = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        history
            .active_entries()
            .iter()
            .find(|entry| entry.id == entry_id || entry.client_id == entry_id)
            .and_then(|entry| entry.files.iter().find(|file| file.id == file_id))
            .and_then(|file| file.local_path.clone())
            .ok_or_else(|| "本机文件路径不可用".to_string())?
    };
    let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
    let modified_at = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default();
    Ok(FileUploadSource {
        full_path: path,
        size: metadata.len(),
        modified_at,
    })
}

fn safe_file_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[tauri::command(rename_all = "camelCase")]
fn begin_file_download(
    state: State<'_, AppState>,
    transfer_id: String,
    file_name: String,
    expected_size: u64,
) -> Result<(), String> {
    let cache_dir = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        active_cache_dir(&state, &history)
    };
    fs::create_dir_all(&cache_dir).map_err(|error| error.to_string())?;
    let path = cache_dir.join(format!(
        "{}.{}.part",
        transfer_id,
        safe_file_name(&file_name)
    ));
    fs::File::create(&path).map_err(|error| error.to_string())?;
    state
        .downloads
        .lock()
        .map_err(|error| error.to_string())?
        .insert(
            transfer_id,
            DownloadState {
                path,
                expected_size,
                received_size: 0,
            },
        );
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
fn append_file_download(
    state: State<'_, AppState>,
    transfer_id: String,
    data: String,
) -> Result<(), String> {
    let bytes = BASE64.decode(data).map_err(|error| error.to_string())?;
    let mut downloads = state.downloads.lock().map_err(|error| error.to_string())?;
    let download = downloads
        .get_mut(&transfer_id)
        .ok_or_else(|| "文件下载任务不存在".to_string())?;
    download.received_size += bytes.len() as u64;
    if download.received_size > download.expected_size {
        return Err("下载内容超过声明大小".to_string());
    }
    fs::OpenOptions::new()
        .append(true)
        .open(&download.path)
        .and_then(|mut file| file.write_all(&bytes))
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn finish_file_download(
    state: State<'_, AppState>,
    transfer_id: String,
    entry_id: String,
    file_id: String,
    file_name: String,
) -> Result<(), String> {
    let download = state
        .downloads
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&transfer_id)
        .ok_or_else(|| "文件下载任务不存在".to_string())?;
    if download.received_size != download.expected_size {
        let _ = fs::remove_file(download.path);
        return Err("文件下载不完整".to_string());
    }
    let cache_dir = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        let cache_dir = active_cache_dir(&state, &history);
        if history
            .active_entries()
            .iter()
            .any(|entry| entry.id == entry_id && entry.kind == "image")
        {
            cache_dir.join("images")
        } else {
            cache_dir
        }
    };
    fs::create_dir_all(&cache_dir).map_err(|error| error.to_string())?;
    let final_path = cache_dir.join(format!("{}-{}", file_id, safe_file_name(&file_name)));
    if final_path.exists() {
        fs::remove_file(&final_path).map_err(|error| error.to_string())?;
    }
    fs::rename(download.path, &final_path).map_err(|error| error.to_string())?;

    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    let file = history
        .active_entries_mut()
        .iter_mut()
        .find(|entry| entry.id == entry_id)
        .and_then(|entry| entry.files.iter_mut().find(|file| file.id == file_id))
        .ok_or_else(|| "剪贴板文件记录不存在".to_string())?;
    file.local_path = Some(final_path.display().to_string());
    file.available = true;
    save_active_history(&state, &history)
}

#[tauri::command(rename_all = "camelCase")]
fn cancel_file_download(state: State<'_, AppState>, transfer_id: String) -> Result<(), String> {
    if let Some(download) = state
        .downloads
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&transfer_id)
    {
        let _ = fs::remove_file(download.path);
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
fn save_entry_files(state: State<'_, AppState>, entry_id: String) -> Result<usize, String> {
    let files = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        let entry = history
            .active_entries()
            .iter()
            .find(|entry| entry.id == entry_id)
            .ok_or_else(|| "剪贴板记录不存在".to_string())?;
        if entry.files.is_empty() {
            return Err("该记录不包含可另存的文件".to_string());
        }
        entry
            .files
            .iter()
            .map(|file| {
                let path = file
                    .local_path
                    .as_ref()
                    .filter(|path| Path::new(path).is_file())
                    .ok_or_else(|| format!("本机文件不可用：{}", file.name))?;
                Ok((file.name.clone(), PathBuf::from(path)))
            })
            .collect::<Result<Vec<_>, String>>()?
    };

    let mut saved = 0;
    for (file_name, source_path) in files {
        let Some(destination_path) = rfd::FileDialog::new()
            .set_file_name(&file_name)
            .save_file()
        else {
            continue;
        };

        let source_path = fs::canonicalize(&source_path).map_err(|error| error.to_string())?;
        if destination_path.exists()
            && fs::canonicalize(&destination_path)
                .map(|path| path == source_path)
                .unwrap_or(false)
        {
            continue;
        }
        fs::copy(&source_path, &destination_path).map_err(|error| {
            format!("无法保存 {}：{}", file_name, error)
        })?;
        saved += 1;
    }
    Ok(saved)
}

#[tauri::command(rename_all = "camelCase")]
fn paste_entry(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<(), String> {
    let payload = {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let entry = history
            .active_entries()
            .iter()
            .find(|entry| entry.id == entry_id)
            .cloned()
            .ok_or_else(|| "clipboard entry was not found".to_string())?;
        let payload = if entry.kind == "files" {
            let paths = entry
                .files
                .iter()
                .map(|file| {
                    let path = file
                        .local_path
                        .as_ref()
                        .ok_or_else(|| format!("文件 {} 仅存在于其他设备", file.name))?;
                    if !Path::new(path).exists() {
                        return Err(format!("文件已不存在：{}", file.name));
                    }
                    Ok(path.clone())
                })
                .collect::<Result<Vec<_>, String>>()?;
            let signature_paths = paths.iter().map(PathBuf::from).collect::<Vec<_>>();
            history.last_file_signature = file_signature(&signature_paths);
            history.last_clipboard.clear();
            ClipboardPayload::Files(paths)
        } else if entry.kind == "image" {
            let image_path = entry
                .files
                .first()
                .and_then(|file| file.local_path.as_deref())
                .ok_or_else(|| "图片文件不可用".to_string())?;
            let image = fs::read(image_path).map_err(|error| error.to_string())?;
            history.last_image_signature = image_signature(&image);
            history.last_clipboard.clear();
            history.last_file_signature.clear();
            ClipboardPayload::Image(image)
        } else {
            let rich_text = RichText {
                text: entry.content,
                html: entry.html,
                rtf: entry.rtf,
            };
            history.last_clipboard = rich_text_signature(&rich_text);
            history.last_file_signature.clear();
            history.last_image_signature.clear();
            ClipboardPayload::Text(rich_text)
        };
        save_active_history(&state, &history)?;
        payload
    };
    match payload {
        ClipboardPayload::Text(rich_text) => write_clipboard_text(&app, &rich_text)?,
        ClipboardPayload::Files(paths) => write_clipboard_files(&paths)?,
        ClipboardPayload::Image(image) => write_clipboard_image(&image)?,
    }
    window.hide().map_err(|error| error.to_string())?;
    thread::sleep(Duration::from_millis(90));
    synthesize_paste()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| error.to_string())?;
            let histories_dir = app_data_dir.join("histories");
            let sync_config_path = app_data_dir.join("sync-config.json");
            let sync_config = load_sync_config(&sync_config_path);
            let history_key = sync_config
                .as_ref()
                .map(history_key_for_config)
                .unwrap_or_else(default_active_history);
            let mut history = load_history(&history_path_for_key(&histories_dir, &history_key), &history_key);
            retain_single_history(&mut history, &history_key);
            save_history(&history_path_for_key(&histories_dir, &history_key), &history)?;
            app.manage(AppState {
                history: Mutex::new(history),
                histories_dir,
                sync_config: Mutex::new(sync_config),
                sync_config_path,
                downloads: Mutex::new(HashMap::new()),
            });
            setup_tray(app.handle())?;
            start_clipboard_monitor(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            tauri::WindowEvent::Focused(false) if window.label() == "paste" => {
                let _ = window.hide();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            list_entries,
            get_device,
            configure_device,
            get_sync_config,
            open_app_data_dir,
            save_sync_config,
            upsert_remote_entry,
            set_pinned,
            delete_entry,
            clear_history,
            open_paste,
            hide_paste,
            hide_main,
            get_missing_files,
            get_file_upload_source,
            read_file_chunk,
            begin_file_download,
            append_file_download,
            finish_file_download,
            cancel_file_download,
            save_entry_files,
            paste_entry
        ])
        .run(tauri::generate_context!())
        .expect("error while running ClipRoam");
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgba, RgbaImage};

    #[test]
    fn screenshot_webp_round_trip_preserves_pixels() {
        let source = RgbaImage::from_fn(16, 12, |x, y| {
            Rgba([(x * 13) as u8, (y * 19) as u8, ((x + y) * 7) as u8, 255])
        });
        let mut bmp = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(source.clone())
            .write_to(&mut bmp, ImageFormat::Bmp)
            .unwrap();

        let (webp, width, height) = encode_image_as_webp(&bmp.into_inner()).unwrap();
        assert_eq!((width, height), (16, 12));
        assert_eq!(image::guess_format(&webp).unwrap(), ImageFormat::WebP);

        let restored_bmp = decode_image_as_bmp(&webp).unwrap();
        let restored = image::load_from_memory_with_format(&restored_bmp, ImageFormat::Bmp)
            .unwrap()
            .to_rgba8();
        assert_eq!(restored, source);
    }

    #[test]
    fn history_database_supports_full_text_search() {
        let directory = std::env::temp_dir().join(format!("cliproam-history-test-{}", Uuid::new_v4()));
        let path = directory.join("history.sqlite");
        let connection = open_history_database(&path).expect("create history database");
        connection
            .execute(
                "INSERT INTO entries_fts (entry_id, content, file_names, pinyin) VALUES (?, ?, ?, ?)",
                params!["entry-1", "ClipRoam local history", "note.txt", "cliproam"],
            )
            .expect("insert search entry");
        let matches: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM entries_fts WHERE entries_fts MATCH 'cliproam'",
                [],
                |row| row.get(0),
            )
            .expect("query full text index");
        drop(connection);
        fs::remove_dir_all(&directory).expect("remove temporary history database");
        assert_eq!(matches, 1);
    }
}
