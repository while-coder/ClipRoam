mod content;
#[cfg(any(target_os = "macos", target_os = "linux", test))]
mod platform_clipboard;
mod store;
#[cfg(target_os = "windows")]
mod virtual_files;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use image::{GenericImageView, ImageFormat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(target_os = "windows")]
use std::time::Instant;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{mpsc, Condvar, Mutex},
    thread,
};
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use std::{process::Command, time::Duration};
use tauri::{AppHandle, Emitter, Manager, State};
#[cfg(target_os = "android")]
use tauri_plugin_cliproam_share_receiver::{PendingShare, ShareReceiverExt};
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use tauri::{
    menu::{Menu, MenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    PhysicalPosition, Position, Size, WebviewWindowBuilder,
};
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
use tauri_plugin_clipboard_manager::ClipboardExt;

use content::{
    collect_tree, describe_roots, download_path, file_entry_signature, file_signature,
    hash_bytes, hash_file, local_source_was_lost, preserve_local_sources, readable_path,
    rebuild_tree, tree_contents, tree_parent_at_path, upload_image_path,
    ClipboardEntry, ClipboardEntryExtra, ImageInfo, LocalSources, TreeNode,
};
use store::{
    cache_dir_for, cached_hash, collect_local_garbage, default_active_history, history_path_for_key,
    load_history, open_history_database, refresh_entry_summary,
    register_cached_file, remember_hash, retain_single_history, save_history, trim_history,
    write_entry_data, HistoryData, LOCAL_HISTORY_KEY,
};

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
const TRAY_SHOW_MAIN: &str = "show-main";
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
const TRAY_QUIT: &str = "quit";
const FILE_CHUNK_LIMIT: usize = 128 * 1024;
const THUMBNAIL_MAX_EDGE: u32 = 64;
const THUMBNAIL_MAX_BYTES: usize = 72 * 1024;
/// How many freshly hashed paths are folded into the entry before the UI is
/// told about the progress.
const HASH_PROGRESS_BATCH: usize = 32;

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
    #[serde(default = "default_auto_receive_clipboard")]
    auto_receive_clipboard: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlatformCapabilities {
    mobile: bool,
    clipboard_monitoring: bool,
    global_shortcut: bool,
    automatic_paste: bool,
    file_clipboard: bool,
    image_clipboard: bool,
    native_file_export: bool,
    open_data_directory: bool,
    share_receiver: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToastPayload {
    message: String,
    tone: String,
}

#[tauri::command]
fn get_platform_capabilities() -> PlatformCapabilities {
    let mobile = cfg!(any(target_os = "android", target_os = "ios"));
    PlatformCapabilities {
        mobile,
        clipboard_monitoring: !mobile,
        global_shortcut: !mobile,
        automatic_paste: !mobile,
        file_clipboard: !mobile,
        image_clipboard: !mobile,
        native_file_export: !mobile,
        open_data_directory: !mobile,
        share_receiver: cfg!(target_os = "android"),
    }
}

fn default_server_protocol() -> String {
    "http".to_string()
}

fn default_auto_upload_limit_mb() -> u64 {
    10
}

fn default_auto_receive_clipboard() -> bool {
    true
}

struct AppState {
    history: Mutex<HistoryData>,
    histories_dir: PathBuf,
    sync_config: Mutex<Option<SyncConfig>>,
    sync_config_path: PathBuf,
    downloads: Mutex<HashMap<String, DownloadState>>,
    save_sessions: Mutex<HashMap<String, SaveSession>>,
    virtual_downloads: VirtualDownloads,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    platform_clipboard: platform_clipboard::PlatformClipboard,
    /// `Sender` is not `Sync`, so managed state has to guard it.
    hash_queue: Mutex<mpsc::Sender<String>>,
    share_import: Mutex<()>,
    #[cfg(target_os = "windows")]
    paste_drag_focus_guard: Mutex<Option<Instant>>,
}

#[derive(Default)]
struct VirtualDownloadStatus {
    requested: bool,
    complete: bool,
    error: Option<String>,
}

#[derive(Default)]
struct VirtualDownloads {
    transfers: Mutex<HashMap<String, VirtualDownloadStatus>>,
    changed: Condvar,
}

impl VirtualDownloads {
    fn request(&self, file_id: &str) -> bool {
        let Ok(mut transfers) = self.transfers.lock() else { return false };
        let status = transfers.entry(file_id.to_string()).or_default();
        if status.complete {
            return false;
        }
        if status.error.take().is_some() {
            status.requested = false;
        }
        if status.requested {
            false
        } else {
            status.requested = true;
            true
        }
    }

    fn begin(&self, file_id: &str) {
        if let Ok(mut transfers) = self.transfers.lock() {
            transfers.insert(file_id.to_string(), VirtualDownloadStatus {
                requested: true,
                complete: false,
                error: None,
            });
            self.changed.notify_all();
        }
    }

    fn progress(&self) {
        self.changed.notify_all();
    }

    fn complete(&self, file_id: &str) {
        if let Ok(mut transfers) = self.transfers.lock() {
            let status = transfers.entry(file_id.to_string()).or_default();
            status.complete = true;
            status.error = None;
            self.changed.notify_all();
        }
    }

    fn fail(&self, file_id: &str, error: String) {
        if let Ok(mut transfers) = self.transfers.lock() {
            let status = transfers.entry(file_id.to_string()).or_default();
            status.complete = false;
            status.error = Some(error);
            self.changed.notify_all();
        }
    }
}

struct DownloadState {
    path: PathBuf,
    file_id: String,
    expected_size: u64,
    received_size: u64,
    hasher: Sha256,
    target: DownloadTarget,
}

enum DownloadTarget {
    Cache,
    Save {
        save_id: String,
        completed_path: PathBuf,
    },
}

struct SaveSession {
    entry_id: String,
    destination: PathBuf,
    staging_dir: PathBuf,
    single_file: bool,
    expected: HashMap<String, u64>,
    in_progress: HashSet<String>,
    downloaded: HashSet<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MissingFile {
    file_id: String,
    size: u64,
    source_device_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SavePreparation {
    save_id: String,
    missing: Vec<MissingFile>,
}

#[derive(Debug, Clone)]
struct RichText {
    text: String,
    html: Option<String>,
    rtf: Option<String>,
}

struct PendingHash {
    path: String,
    source: String,
    size: u64,
    modified_at: Option<u64>,
}

/// The business flow is shared; only the final system clipboard delivery
/// differs. Windows can expose remote contents lazily, while macOS/Linux need
/// real local paths before they can publish a file list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilePasteStrategy {
    VirtualStream,
    MaterializedPaths,
}

impl FilePasteStrategy {
    fn for_entry(entry: &ClipboardEntry) -> Self {
        #[cfg(target_os = "windows")]
        if entry.kind == "files" && virtual_files::supports_entry(entry) {
            return Self::VirtualStream;
        }
        let _ = entry;
        Self::MaterializedPaths
    }

    fn requires_complete_content(self, kind: &str) -> bool {
        kind == "image" || (kind == "files" && self == Self::MaterializedPaths)
    }
}

fn history_key_for_config(config: &SyncConfig) -> String {
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

fn load_sync_config(path: &Path) -> Option<SyncConfig> {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

fn write_sync_config(path: &Path, config: &Option<SyncConfig>) -> Result<(), String> {
    let json = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

fn save_active_history(state: &AppState, history: &HistoryData) -> Result<(), String> {
    save_history(
        &history_path_for_key(&state.histories_dir, &history.active_history),
        history,
    )
}

fn active_cache_dir(state: &AppState, history: &HistoryData) -> PathBuf {
    cache_dir_for(&state.histories_dir, &history.active_history)
}

/// The frontend renders lists of hundreds of entries; shipping their trees
/// would mean tens of thousands of nodes per refresh.
fn lightweight_entry(entry: &ClipboardEntry) -> ClipboardEntry {
    // html/rtf can be hundreds of kilobytes per rich-text entry and the list
    // never renders them, so they stay behind `get_entry`.
    let mut lightweight = ClipboardEntry {
        file_info: None,
        image_info: None,
        html: None,
        rtf: None,
        sources: LocalSources::default(),
        ..entry.clone()
    };
    if lightweight.kind == "files" {
        if let Some(file_info) = &entry.file_info {
            lightweight.content = describe_roots(file_info);
        }
    }
    lightweight
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

fn rich_text_signature(rich_text: &RichText) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    // arboard wraps HTML on macOS to force UTF-8 interpretation. Treat that
    // transport wrapper as equivalent to the original fragment so paste does
    // not get captured back as a second history item.
    const MAC_HTML_PREFIX: &str =
        "<html><head><meta http-equiv=\"content-type\" content=\"text/html; charset=utf-8\"></head><body>";
    const MAC_HTML_SUFFIX: &str = "</body></html>";
    let html = rich_text.html.as_deref().map(|html| {
        html.strip_prefix(MAC_HTML_PREFIX)
            .and_then(|html| html.strip_suffix(MAC_HTML_SUFFIX))
            .unwrap_or(html)
    });
    for value in [
        Some(rich_text.text.as_str()),
        html,
        rich_text.rtf.as_deref(),
    ] {
        for byte in value.unwrap_or_default().bytes().chain(std::iter::once(0)) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    format!("{hash:016x}")
}

fn image_signature(image: &[u8]) -> String {
    // Clipboard encodings differ by platform (BMP, PNG, TIFF), while pasted
    // history images are WebP. Hash canonical RGBA pixels so writing an image
    // does not make the monitor capture the same pixels as a new entry.
    let canonical = image::load_from_memory(image).ok().map(|decoded| {
        let rgba = decoded.into_rgba8();
        let (width, height) = rgba.dimensions();
        (width, height, rgba.into_raw())
    });
    let (prefix, bytes) = match canonical {
        Some((width, height, pixels)) => (format!("{width}x{height}"), pixels),
        None => (image.len().to_string(), image.to_vec()),
    };
    // FNV-1a is sufficient here: this only suppresses repeated reads of the current clipboard.
    let hash = bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    format!("{prefix}:{hash:016x}")
}

/// Local, pre-publish entry identity: the seq of the capture's durable queue
/// row. The server assigns the real id when the entry is first published and
/// `apply_published_entry` swaps it out, so this only has to stay stable until
/// then.
fn new_entry(seq: i64, kind: &str, content: String, device_id: String) -> ClipboardEntry {
    ClipboardEntry {
        id: store::temp_entry_id(seq),
        kind: kind.to_string(),
        content,
        html: None,
        rtf: None,
        file_info: None,
        image_info: None,
        source_device_id: device_id,
        created_at: Utc::now().to_rfc3339(),
        summary: Default::default(),
        sources: LocalSources::default(),
    }
}

/// Writes the full capture payload to the durable upload queue. The returned
/// seq doubles as the entry's temporary local id.
fn queue_entry_payload(
    state: &AppState,
    history: &HistoryData,
    kind: &str,
    content: &str,
    extra: &ClipboardEntryExtra,
    created_at: &str,
) -> Result<i64, String> {
    let payload = serde_json::to_string(extra).map_err(|error| error.to_string())?;
    store::enqueue_pending_entry(
        &history_path_for_key(&state.histories_dir, &history.active_history),
        kind,
        content,
        &payload,
        created_at,
    )
}

/// Re-serializes an entry's large fields the way `save_history` stores them,
/// so queue rows and entry rows carry identical payloads.
fn entry_extra(entry: &ClipboardEntry) -> Result<String, String> {
    serde_json::to_string(&ClipboardEntryExtra {
        html: entry.html.clone(),
        rtf: entry.rtf.clone(),
        file_info: entry.file_info.clone(),
        image_info: entry.image_info.clone(),
    })
    .map_err(|error| error.to_string())
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
        let device_id = history.device_id.clone();
        let created_at = Utc::now().to_rfc3339();
        let extra = ClipboardEntryExtra {
            html: rich_text.html.clone(),
            rtf: rich_text.rtf.clone(),
            file_info: None,
            image_info: None,
        };
        // The payload lands in the durable queue first: the seq it gets back
        // is the entry's local id until the server's id is adopted. Without a
        // queue row there is nothing to sync later, so the capture is skipped.
        let seq = match queue_entry_payload(&state, &history, "text", &rich_text.text, &extra, &created_at) {
            Ok(seq) => seq,
            Err(error) => {
                eprintln!("ClipRoam: 记录待上传条目失败：{error}");
                return Ok(());
            }
        };
        history.last_clipboard = signature;
        history.last_file_signature.clear();
        history.last_image_signature.clear();
        let mut entry = new_entry(seq, "text", rich_text.text, device_id);
        entry.html = extra.html;
        entry.rtf = extra.rtf;
        entry.created_at = created_at;
        let entries = history.active_entries_mut();
        entries.retain(|item| item.content != entry.content);
        entries.insert(0, entry.clone());
        trim_history(entries);
        save_active_history(&state, &history)?;
        entry
    };
    // Text has no contents to hash, so it is publishable the moment it lands —
    // the frontend drains the queue whenever an entry becomes ready.
    app.emit("cliproam://entry-created", lightweight_entry(&entry))
        .map_err(|error| error.to_string())?;
    app.emit("cliproam://entry-ready", entry.id)
        .map_err(|error| error.to_string())
}

fn capture_files(app: &AppHandle, paths: Vec<PathBuf>) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let signature = file_signature(&paths);
    let state = app.state::<AppState>();
    // Walking a large folder can take seconds, so the duplicate check happens
    // before the tree is collected and the history lock is released for it.
    if state
        .history
        .lock()
        .map_err(|error| error.to_string())?
        .last_file_signature
        == signature
    {
        return Ok(());
    }
    let collected = collect_tree(&paths)?;
    let (entry, entry_id) = {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        if history.last_file_signature == signature {
            return Ok(());
        }
        history.last_file_signature = signature.clone();
        history.last_clipboard.clear();
        history.last_image_signature.clear();
        let cache_dir = active_cache_dir(&state, &history);
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        let device_id = history.device_id.clone();
        let created_at = Utc::now().to_rfc3339();
        let content = describe_roots(&collected.file_info);
        let entries = history.active_entries_mut();
        let entry_id = match entries
            .iter()
            .position(|item| item.kind == "files" && file_entry_signature(item) == signature)
        {
            Some(index) => {
                let mut existing = entries.remove(index);
                existing.created_at = created_at;
                // A reused entry keeps its id — and its queue row when it is
                // still unpublished. Recreate the row if it went missing, so
                // the copy is not silently never synced.
                if let Some(seq) = store::temp_entry_seq(&existing.id) {
                    let payload = entry_extra(&existing)?;
                    if let Err(error) = store::ensure_pending_entry(
                        &history_path,
                        seq,
                        &existing.kind,
                        &existing.content,
                        &payload,
                        &existing.created_at,
                    ) {
                        eprintln!("ClipRoam: 记录待上传条目失败：{error}");
                    }
                }
                let entry_id = existing.id.clone();
                entries.insert(0, existing);
                entry_id
            }
            None => {
                // The tree goes into the queue with unresolved content ids
                // (`f: ""`); the hash worker folds the real ones back in.
                let extra = ClipboardEntryExtra {
                    html: None,
                    rtf: None,
                    file_info: Some(collected.file_info.clone()),
                    image_info: None,
                };
                let payload = serde_json::to_string(&extra).map_err(|error| error.to_string())?;
                let seq = match store::enqueue_pending_entry(&history_path, "files", &content, &payload, &created_at)
                {
                    Ok(seq) => seq,
                    Err(error) => {
                        eprintln!("ClipRoam: 记录待上传条目失败：{error}");
                        return Ok(());
                    }
                };
                let mut entry = new_entry(seq, "files", content, device_id);
                entry.created_at = created_at;
                entry.file_info = Some(collected.file_info);
                entry.sources = collected.sources;
                let entry_id = entry.id.clone();
                entries.insert(0, entry);
                entry_id
            }
        };
        trim_history(entries);
        refresh_entry_summary(&mut history, &entry_id, &cache_dir);
        save_active_history(&state, &history)?;
        let entry = history
            .find(&entry_id)
            .map(lightweight_entry)
            .ok_or_else(|| "剪贴板记录不存在".to_string())?;
        (entry, entry_id)
    };
    queue_hashing(&state, &entry_id);
    app.emit("cliproam://entry-created", entry)
        .map_err(|error| error.to_string())
}

fn capture_image(app: &AppHandle, image: Vec<u8>) -> Result<(), String> {
    let signature = image_signature(&image);
    let state = app.state::<AppState>();
    if state
        .history
        .lock()
        .map_err(|error| error.to_string())?
        .last_image_signature
        == signature
    {
        return Ok(());
    }
    let (webp, width, height, thumbnail) = encode_image_as_webp(&image)?;
    // The bytes are already in memory, so hashing is immediate and the entry
    // never passes through the background queue.
    let file_id = hash_bytes(&webp);
    let entry = {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        if history.last_image_signature == signature {
            return Ok(());
        }
        let cache_dir = active_cache_dir(&state, &history);
        let image_path = upload_image_path(&cache_dir, &file_id).ok_or_else(|| "内容标识不合法".to_string())?;
        if let Some(parent) = image_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        if !image_path.is_file() {
            fs::write(&image_path, &webp).map_err(|error| error.to_string())?;
        }
        register_cached_file(
            &history_path_for_key(&state.histories_dir, &history.active_history),
            &file_id,
            webp.len() as u64,
        )?;
        history.cached_files.insert(file_id.clone());
        history.last_image_signature = signature;
        history.last_clipboard.clear();
        history.last_file_signature.clear();

        let device_id = history.device_id.clone();
        let created_at = Utc::now().to_rfc3339();
        let content = format!("截图（{width} × {height}）");
        let image_info = ImageInfo {
            file_id: file_id.clone(),
            size: webp.len() as u64,
            thumbnail: thumbnail.unwrap_or_default(),
        };
        // Bytes are hashed already, so the queued payload is complete and the
        // entry is publishable as soon as it lands.
        let extra = ClipboardEntryExtra {
            html: None,
            rtf: None,
            file_info: None,
            image_info: Some(image_info.clone()),
        };
        let payload = serde_json::to_string(&extra).map_err(|error| error.to_string())?;
        let seq = match store::enqueue_pending_entry(
            &history_path_for_key(&state.histories_dir, &history.active_history),
            "image",
            &content,
            &payload,
            &created_at,
        ) {
            Ok(seq) => seq,
            Err(error) => {
                eprintln!("ClipRoam: 记录待上传条目失败：{error}");
                return Ok(());
            }
        };
        let mut entry = new_entry(seq, "image", content, device_id);
        entry.created_at = created_at;
        entry.image_info = Some(image_info);
        let entry_id = entry.id.clone();
        let entries = history.active_entries_mut();
        entries.insert(0, entry);
        trim_history(entries);
        refresh_entry_summary(&mut history, &entry_id, &cache_dir);
        save_active_history(&state, &history)?;
        history
            .find(&entry_id)
            .map(lightweight_entry)
            .ok_or_else(|| "剪贴板记录不存在".to_string())?
    };
    let entry_id = entry.id.clone();
    app.emit("cliproam://entry-created", entry)
        .map_err(|error| error.to_string())?;
    app.emit("cliproam://entry-ready", entry_id)
        .map_err(|error| error.to_string())
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn start_clipboard_monitor(app: AppHandle) {
    thread::spawn(move || {
        // Every pass reads the full clipboard and re-decodes its contents for
        // the signatures (a bitmap can be tens of megabytes), so Windows gates
        // each pass on the clipboard sequence number: an unchanged clipboard —
        // a screenshot sitting idle for hours, for example — is skipped without
        // even opening it.
        #[cfg(target_os = "windows")]
        let mut last_clipboard_sequence = 0u32;
        loop {
            #[cfg(target_os = "windows")]
            {
                let sequence = unsafe {
                    windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber()
                };
                if sequence != 0 && sequence == last_clipboard_sequence {
                    thread::sleep(Duration::from_millis(350));
                    continue;
                }
                last_clipboard_sequence = sequence;
            }
            if let Some(paths) = read_clipboard_files(&app).filter(|paths| !paths.is_empty()) {
                let _ = capture_files(&app, paths);
            } else if let Some(rich_text) = read_clipboard_text(&app) {
                let _ = capture_text(&app, rich_text);
            } else if let Some(image) = read_clipboard_image(&app) {
                let _ = capture_image(&app, image);
            }
            thread::sleep(Duration::from_millis(350));
        }
    });
}

#[tauri::command]
fn capture_current_clipboard_text(app: AppHandle) -> Result<bool, String> {
    let Some(rich_text) = read_clipboard_text(&app) else {
        return Ok(false);
    };
    capture_text(&app, rich_text)?;
    Ok(true)
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareImportSummary {
    shares: usize,
    texts: usize,
    images: usize,
    files: usize,
}

#[cfg(target_os = "android")]
fn persist_shared_files(app: &AppHandle, share: &PendingShare) -> Result<Vec<PathBuf>, String> {
    let state = app.state::<AppState>();
    let cache_dir = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        active_cache_dir(&state, &history)
    };
    let request_id = uuid::Uuid::parse_str(&share.id).map_err(|_| "分享请求标识不合法".to_string())?;
    let directory = cache_dir.join("share").join(request_id.to_string());
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;

    let mut paths = Vec::with_capacity(share.items.len());
    for (index, item) in share.items.iter().enumerate() {
        let source = PathBuf::from(&item.path);
        if !source.is_file() {
            return Err(format!("分享文件已失效：{}", item.name));
        }
        let name = Path::new(&item.name)
            .file_name()
            .filter(|name| !name.is_empty())
            .map(|name| name.to_owned())
            .unwrap_or_else(|| format!("shared-{}", index + 1).into());
        let target = directory.join(name);
        let source_size = fs::metadata(&source).map_err(|error| error.to_string())?.len();
        let target_matches = fs::metadata(&target)
            .map(|metadata| metadata.is_file() && metadata.len() == source_size)
            .unwrap_or(false);
        if !target_matches {
            fs::copy(&source, &target).map_err(|error| format!("无法保存分享文件 {}：{error}", item.name))?;
        }
        paths.push(target);
    }
    Ok(paths)
}

#[cfg(target_os = "android")]
fn import_android_share(app: &AppHandle, share: &PendingShare) -> Result<ShareImportSummary, String> {
    let mut summary = ShareImportSummary::default();
    if let Some(text) = share.text.as_ref().filter(|text| !text.trim().is_empty()) {
        let rich_text = RichText {
            text: text.clone(),
            html: share.html.clone(),
            rtf: None,
        };
        // A share is the mobile equivalent of a fresh local clipboard capture.
        // Keep the OS clipboard useful too, but never discard the history item
        // just because a particular device rejected the clipboard write.
        let _ = write_clipboard_text(app, &rich_text);
        capture_text(app, rich_text)?;
        summary.texts = 1;
    }

    if share.items.len() == 1 && share.items[0].mime_type.starts_with("image/") {
        let image = fs::read(&share.items[0].path)
            .map_err(|error| format!("无法读取分享图片：{error}"))?;
        capture_image(app, image)?;
        summary.images = 1;
    } else if !share.items.is_empty() {
        let paths = persist_shared_files(app, share)?;
        capture_files(app, paths)?;
        summary.files = share.items.len();
    }
    summary.shares = 1;
    Ok(summary)
}

#[tauri::command]
fn consume_mobile_shares(app: AppHandle, state: State<'_, AppState>) -> Result<ShareImportSummary, String> {
    let _guard = state.share_import.lock().map_err(|error| error.to_string())?;
    #[cfg(target_os = "android")]
    {
        let mut imported = ShareImportSummary::default();
        for share in app.share_receiver().pending().map_err(|error| error.to_string())? {
            let summary = import_android_share(&app, &share)?;
            app.share_receiver()
                .acknowledge(&share.id)
                .map_err(|error| error.to_string())?;
            imported.shares += summary.shares;
            imported.texts += summary.texts;
            imported.images += summary.images;
            imported.files += summary.files;
        }
        Ok(imported)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(ShareImportSummary::default())
    }
}

fn queue_hashing(state: &AppState, entry_id: &str) {
    if let Ok(sender) = state.hash_queue.lock() {
        let _ = sender.send(entry_id.to_string());
    }
}

fn pending_entry_ids(history: &HistoryData) -> Vec<String> {
    history
        .active_entries()
        .iter()
        .filter(|entry| entry.sources.files.iter().any(|source| source.file_id.is_none()))
        .map(|entry| entry.id.clone())
        .collect()
}

/// Hashing runs on one background thread: an entry becomes visible and pasteable
/// straight away, and only reaches the server once every content is identified.
fn start_hash_worker(app: AppHandle, receiver: mpsc::Receiver<String>) {
    thread::spawn(move || {
        for entry_id in receiver {
            if let Err(error) = hash_entry_files(&app, &entry_id) {
                eprintln!("ClipRoam: 计算 {entry_id} 的内容标识失败：{error}");
            }
        }
    });
}

fn hash_entry_files(app: &AppHandle, entry_id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (history_key, pending) = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        let Some(entry) = history.find(entry_id) else {
            return Ok(());
        };
        let pending = entry
            .sources
            .files
            .iter()
            .filter(|source| source.file_id.is_none())
            .map(|source| PendingHash {
                path: source.path.clone(),
                source: source.source.clone(),
                size: source.size,
                modified_at: source.modified_at,
            })
            .collect::<Vec<_>>();
        (history.active_history.clone(), pending)
    };
    if pending.is_empty() {
        return app
            .emit("cliproam://entry-ready", entry_id)
            .map_err(|error| error.to_string());
    }

    // A second connection keeps the hash cache off the UI thread's connection.
    let connection = open_history_database(&history_path_for_key(&state.histories_dir, &history_key))?;
    let mut batch = Vec::new();
    for item in pending {
        let modified_at = item.modified_at.map(|value| value as i64).unwrap_or(-1);
        let file_id = cached_hash(&connection, &item.source, item.size, modified_at).or_else(|| {
            // A file that vanished between copy and hash drops out of the tree.
            let hashed = hash_file(Path::new(&item.source)).ok()?;
            remember_hash(&connection, &item.source, item.size, modified_at, &hashed);
            Some(hashed)
        });
        batch.push((item.path, file_id));
        if batch.len() >= HASH_PROGRESS_BATCH {
            if apply_hashes(app, entry_id, &batch, false)?.is_none() {
                return Ok(());
            };
            batch.clear();
        }
    }
    if apply_hashes(app, entry_id, &batch, true)?.is_none() {
        return Ok(());
    };
    app.emit("cliproam://entry-ready", entry_id)
        .map_err(|error| error.to_string())
}

/// Folds resolved content ids into the entry. Only the final call persists, so
/// progress updates stay in memory.
fn apply_hashes(
    app: &AppHandle,
    entry_id: &str,
    resolved: &[(String, Option<String>)],
    persist: bool,
) -> Result<Option<String>, String> {
    let state = app.state::<AppState>();
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    let cache_dir = active_cache_dir(&state, &history);
    let hashes = resolved
        .iter()
        .map(|(path, file_id)| (path.as_str(), file_id.as_deref()))
        .collect::<HashMap<_, _>>();
    let final_entry_id = {
        let Some(entry) = history.find_mut(entry_id) else {
            return Ok(None);
        };
        if let Some(file_info) = entry.file_info.as_mut() {
            for (path, file_id) in resolved {
                let Some(parent) = tree_parent_at_path(file_info, path) else {
                    continue;
                };
                let leaf = path.rsplit('/').next().unwrap_or_default();
                match file_id {
                    Some(file_id) => {
                        if let Some(TreeNode::File { f, .. }) = parent.get_mut(leaf) {
                            *f = file_id.clone();
                        }
                    }
                    // A file that vanished between copy and hash drops out of the tree.
                    None => {
                        parent.shift_remove(leaf);
                    }
                }
            }
        }
        entry.sources.files.retain_mut(|source| match hashes.get(source.path.as_str()) {
            Some(Some(file_id)) => {
                source.file_id = Some((*file_id).to_string());
                true
            }
            Some(None) => false,
            None => true,
        });
        entry.id.clone()
    };
    refresh_entry_summary(&mut history, &final_entry_id, &cache_dir);
    if persist {
        save_active_history(&state, &history)?;
        // The queue row carries the published payload, so the resolved tree
        // must land there too — an unpublished files entry is only synced
        // once its content ids are known.
        if let Some(seq) = store::temp_entry_seq(&final_entry_id) {
            if let Some(entry) = history.find(&final_entry_id) {
                let payload = entry_extra(entry)?;
                if let Err(error) = store::update_pending_entry(
                    &history_path_for_key(&state.histories_dir, &history.active_history),
                    seq,
                    &entry.kind,
                    &entry.content,
                    &payload,
                    &entry.created_at,
                ) {
                    eprintln!("ClipRoam: 回写待上传条目失败：{error}");
                }
            }
        }
    }
    drop(history);
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())?;
    Ok(Some(final_entry_id))
}

#[cfg(target_os = "windows")]
fn read_clipboard_files(_app: &AppHandle) -> Option<Vec<PathBuf>> {
    clipboard_win::get_clipboard(clipboard_win::formats::FileList).ok()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn read_clipboard_files(app: &AppHandle) -> Option<Vec<PathBuf>> {
    app.state::<AppState>().platform_clipboard.read_files()
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn read_clipboard_files(_app: &AppHandle) -> Option<Vec<PathBuf>> {
    None
}

#[cfg(target_os = "windows")]
fn read_clipboard_image(_app: &AppHandle) -> Option<Vec<u8>> {
    use clipboard_win::{formats::Bitmap, Clipboard, Getter};

    let _clipboard = Clipboard::new_attempts(10).ok()?;
    let mut image = Vec::new();
    Bitmap.read_clipboard(&mut image).ok()?;
    (!image.is_empty()).then_some(image)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn read_clipboard_image(app: &AppHandle) -> Option<Vec<u8>> {
    app.state::<AppState>().platform_clipboard.read_image_as_bmp()
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn read_clipboard_image(_app: &AppHandle) -> Option<Vec<u8>> {
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn read_clipboard_text(app: &AppHandle) -> Option<RichText> {
    let clipboard = &app.state::<AppState>().platform_clipboard;
    clipboard.read_text().map(|text| RichText {
        html: clipboard.read_html(),
        text,
        rtf: None,
    })
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn read_clipboard_text(app: &AppHandle) -> Option<RichText> {
    app.clipboard().read_text().ok().and_then(|text| {
        (!text.trim().is_empty()).then_some(RichText { text, html: None, rtf: None })
    })
}

fn encode_image_as_webp(image: &[u8]) -> Result<(Vec<u8>, u32, u32, Option<String>), String> {
    let decoded = image::load_from_memory(image)
        .map_err(|error| error.to_string())?;
    let (width, height) = decoded.dimensions();
    let mut output = Cursor::new(Vec::new());
    decoded
        .write_to(&mut output, ImageFormat::WebP)
        .map_err(|error| error.to_string())?;
    let thumbnail = decoded.thumbnail(THUMBNAIL_MAX_EDGE, THUMBNAIL_MAX_EDGE);
    let mut thumbnail_output = Cursor::new(Vec::new());
    thumbnail
        .write_to(&mut thumbnail_output, ImageFormat::WebP)
        .map_err(|error| error.to_string())?;
    let thumbnail = thumbnail_output.into_inner();
    Ok((
        output.into_inner(),
        width,
        height,
        (thumbnail.len() <= THUMBNAIL_MAX_BYTES).then(|| BASE64.encode(thumbnail)),
    ))
}

fn decode_image_as_bmp(image: &[u8]) -> Result<Vec<u8>, String> {
    let decoded = image::load_from_memory(image).map_err(|error| error.to_string())?;
    let mut output = Cursor::new(Vec::new());
    decoded
        .write_to(&mut output, ImageFormat::Bmp)
        .map_err(|error| error.to_string())?;
    Ok(output.into_inner())
}

#[tauri::command]
fn list_entries(state: State<'_, AppState>) -> Result<Vec<ClipboardEntry>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    Ok(history.active_entries().iter().map(lightweight_entry).collect())
}

/// The full entry, tree included — used when publishing to the server.
#[tauri::command(rename_all = "camelCase")]
fn get_entry(state: State<'_, AppState>, entry_id: String) -> Result<ClipboardEntry, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    history
        .find(&entry_id)
        .cloned()
        .ok_or_else(|| "剪贴板记录不存在".to_string())
}

/// Every content an entry references, whichever kind carries it.
fn entry_contents_of(entry: &ClipboardEntry) -> Vec<(String, u64)> {
    match (&entry.file_info, &entry.image_info) {
        (Some(file_info), _) => tree_contents(file_info),
        (None, Some(image)) => vec![(image.file_id.clone(), image.size)],
        (None, None) => Vec::new(),
    }
}

fn entry_references(entry: &ClipboardEntry, file_id: &str) -> bool {
    entry_contents_of(entry)
        .into_iter()
        .any(|(id, _)| id == file_id)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct EntryFileCandidate {
    file_id: String,
    size: u64,
    uploaded: bool,
}

#[tauri::command(rename_all = "camelCase")]
fn list_entry_files(
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<Vec<EntryFileCandidate>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let entry = history
        .find(&entry_id)
        .ok_or_else(|| "剪贴板记录不存在".to_string())?;
    Ok(entry_contents_of(entry)
        .into_iter()
        .map(|(file_id, size)| EntryFileCandidate {
            uploaded: history.uploaded_files.contains(&file_id),
            file_id,
            size,
        })
        .collect())
}

#[tauri::command]
fn get_device(state: State<'_, AppState>) -> Result<(String, String), String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    Ok((history.device_id.clone(), history.device_name.clone()))
}

/// File ids this device knows nothing about: neither a local blob in the cache
/// nor an "available" mark from the server pool. The sync flow queries server
/// storage status only for these, so locally known contents never ride a
/// `/files/query` request.
#[tauri::command(rename_all = "camelCase")]
fn filter_unknown_file_ids(
    state: State<'_, AppState>,
    file_ids: Vec<String>,
) -> Result<Vec<String>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let mut unknown = Vec::new();
    let mut seen = HashSet::new();
    for file_id in file_ids {
        if file_id.is_empty()
            || !seen.insert(file_id.clone())
            || history.cached_files.contains(&file_id)
            || history.uploaded_files.contains(&file_id)
        {
            continue;
        }
        unknown.push(file_id);
    }
    Ok(unknown)
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
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let _ = app;
        return Err("移动端应用数据由系统沙箱管理，不能直接打开数据目录".to_string());
    }

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    {
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
        #[cfg(target_os = "linux")]
        Command::new("xdg-open")
            .arg(&app_data_dir)
            .spawn()
            .map_err(|error| error.to_string())?;

        Ok(())
    }
}

#[tauri::command(rename_all = "camelCase")]
fn save_sync_config(
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
        pending_entry_ids(&history)
    };
    for entry_id in pending {
        queue_hashing(&state, &entry_id);
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
    available_file_ids: Vec<String>,
) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let cache_dir = active_cache_dir(&state, &history);
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        // The caller pre-queried which contents the server's pool holds
        // (POST /files/query). Those need no re-upload from this device, and
        // the availability row makes the state survive a restart.
        let available: Vec<String> = entry_contents_of(&entry)
            .into_iter()
            .map(|(file_id, _)| file_id)
            .filter(|file_id| available_file_ids.contains(file_id))
            .collect();
        let entries = history.active_entries_mut();
        if let Some(local) = entries.iter().find(|item| item.id == entry.id) {
            preserve_local_sources(&mut entry, local);
        }
        entries.retain(|item| item.id != entry.id);
        let entry_id = entry.id.clone();
        entries.push(entry);
        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        trim_history(entries);
        refresh_entry_summary(&mut history, &entry_id, &cache_dir);
        save_active_history(&state, &history)?;
        // Existing rows keep their large data during the general history save;
        // a remote update replaces it explicitly here.
        if let Some(entry) = history.find(&entry_id) {
            let connection = open_history_database(&history_path)?;
            write_entry_data(&connection, entry)?;
            store::mark_files_uploaded(&connection, &available);
        }
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

/// Reconciling a fresh install can deliver hundreds of remote entries at once;
/// a single lock, save and event keeps that from locking up the windows.
#[tauri::command(rename_all = "camelCase")]
fn upsert_remote_entries(
    app: AppHandle,
    state: State<'_, AppState>,
    entries: Vec<ClipboardEntry>,
    available_file_ids: Vec<String>,
) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let cache_dir = active_cache_dir(&state, &history);
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        let mut upserted_ids = Vec::with_capacity(entries.len());
        // The caller pre-queried which contents the server's pool holds
        // (POST /files/query). Those need no re-upload from this device, and
        // the availability rows make the state survive a restart.
        let available: HashSet<String> = entries
            .iter()
            .flat_map(entry_contents_of)
            .map(|(file_id, _)| file_id)
            .filter(|file_id| available_file_ids.contains(file_id))
            .collect();
        history.uploaded_files.extend(available.iter().cloned());
        {
            let slot = history.active_entries_mut();
            for mut entry in entries {
                if let Some(local) = slot.iter().find(|item| item.id == entry.id) {
                    preserve_local_sources(&mut entry, local);
                }
                let entry_id = entry.id.clone();
                slot.retain(|item| item.id != entry.id);
                slot.push(entry);
                upserted_ids.push(entry_id);
            }
            slot.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            trim_history(slot);
        }
        for entry_id in &upserted_ids {
            refresh_entry_summary(&mut history, entry_id, &cache_dir);
        }
        save_active_history(&state, &history)?;
        // Existing rows keep their large data during the general history save;
        // a remote update replaces it explicitly here.
        let connection = open_history_database(&history_path)?;
        for entry_id in &upserted_ids {
            if let Some(entry) = history.find(entry_id) {
                write_entry_data(&connection, entry)?;
            }
        }
        let available_vec = available.into_iter().collect::<Vec<_>>();
        store::mark_files_uploaded(&connection, &available_vec);
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

/// Adopts the server's record for a locally captured entry: the local
/// content-hash id is swapped for the server-assigned one so local history,
/// the pending sets and every entryId command share the server's key space.
/// Returns false when the entry was deleted locally while the publish was in
/// flight — the caller must then delete the server row itself.
#[tauri::command(rename_all = "camelCase")]
fn apply_published_entry(
    app: AppHandle,
    state: State<'_, AppState>,
    local_entry_id: String,
    mut entry: ClipboardEntry,
) -> Result<bool, String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        if history.pending_deletions.remove(&local_entry_id) {
            save_active_history(&state, &history)?;
            return Ok(false);
        }
        // A deletion can land while the publish is in flight — including of an
        // entry that was still unpublished. Without the local entry there is
        // nothing to adopt, and the caller must delete the server row.
        if history.find(&local_entry_id).is_none() {
            return Ok(false);
        }
        let cache_dir = active_cache_dir(&state, &history);
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        if let Some(local) = history.find(&local_entry_id) {
            preserve_local_sources(&mut entry, local);
        }
        // The WS echo may have inserted the server id before the publish
        // response arrived; drop both keys so only one row survives.
        let entries = history.active_entries_mut();
        entries.retain(|item| item.id != local_entry_id && item.id != entry.id);
        let entry_id = entry.id.clone();
        entries.push(entry);
        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        trim_history(entries);
        refresh_entry_summary(&mut history, &entry_id, &cache_dir);
        // The id changed, so the save's INSERT OR IGNORE writes a fresh row
        // and the mark-sweep deletes the old one.
        save_active_history(&state, &history)?;
        if let Some(entry) = history.find(&entry_id) {
            let connection = open_history_database(&history_path)?;
            write_entry_data(&connection, entry)?;
        }
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())?;
    Ok(true)
}

#[tauri::command(rename_all = "camelCase")]
fn mark_files_uploaded(
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
    file_ids: Vec<String>,
) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let cache_dir = active_cache_dir(&state, &history);
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        let uploaded = file_ids.into_iter().collect::<Vec<_>>();
        history.uploaded_files.extend(uploaded.iter().cloned());
        let connection = open_history_database(&history_path)?;
        store::mark_files_uploaded(&connection, &uploaded);
        refresh_entry_summary(&mut history, &entry_id, &cache_dir);
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

/// Server storage is content-addressed, so another device can finish uploading
/// a file after this entry was already received locally. Update every local
/// entry that references the now-available content.
#[tauri::command(rename_all = "camelCase")]
fn mark_file_available(
    app: AppHandle,
    state: State<'_, AppState>,
    file_id: String,
) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let cache_dir = active_cache_dir(&state, &history);
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        if history.uploaded_files.contains(&file_id) {
            return Ok(());
        }
        let changed_ids = history
            .active_entries()
            .iter()
            .filter(|entry| entry_references(entry, &file_id))
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        history.uploaded_files.insert(file_id.clone());
        let connection = open_history_database(&history_path)?;
        store::mark_files_uploaded(&connection, &[file_id]);
        for entry_id in &changed_ids {
            refresh_entry_summary(&mut history, entry_id, &cache_dir);
        }
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn delete_entry(app: AppHandle, state: State<'_, AppState>, entry_id: String) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let existed = history.active_entries().iter().any(|entry| entry.id == entry_id);
        history.active_entries_mut().retain(|entry| entry.id != entry_id);
        if existed {
            // The server only knows published (numeric) ids; a temporary id
            // was never uploaded, so removing it merely drops its queue row
            // on the next save.
            if store::temp_entry_seq(&entry_id).is_none() {
                history.pending_deletions.insert(entry_id.clone());
            }
        }
        save_active_history(&state, &history)?;
        // Dropping references is what frees disk space, so the sweep runs here.
        let _ = collect_local_garbage(&state.histories_dir, &mut history);
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn clear_history(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let deleted = history
            .active_entries()
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        history.active_entries_mut().clear();
        for entry_id in &deleted {
            if store::temp_entry_seq(entry_id).is_none() {
                history.pending_deletions.insert(entry_id.clone());
            }
        }
        save_active_history(&state, &history)?;
        let _ = collect_local_garbage(&state.histories_dir, &mut history);
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

/// Applies a server-confirmed deletion without creating a new local tombstone.
#[tauri::command(rename_all = "camelCase")]
fn remove_remote_entry(app: AppHandle, state: State<'_, AppState>, entry_id: String) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        history.active_entries_mut().retain(|entry| entry.id != entry_id);
        save_active_history(&state, &history)?;
        let _ = collect_local_garbage(&state.histories_dir, &mut history);
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn list_pending_deletions(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let mut pending = history.pending_deletions.iter().cloned().collect::<Vec<_>>();
    pending.sort();
    Ok(pending)
}

#[tauri::command(rename_all = "camelCase")]
fn acknowledge_entry_deletion(state: State<'_, AppState>, entry_id: String) -> Result<(), String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    if history.pending_deletions.remove(&entry_id) {
        save_active_history(&state, &history)?;
    }
    Ok(())
}

/// One durable upload-queue row for the sync client, enriched with the local
/// entry state the publish flow needs.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingQueueRowView {
    seq: i64,
    kind: String,
    content: String,
    extra: serde_json::Value,
    created_at: String,
    /// The temporary id the local entry carries until the server's is adopted.
    local_id: String,
    /// False once the entry was adopted, evicted or deleted — the row only
    /// needs acknowledging then.
    exists: bool,
    /// Files entries are publishable only once every content id is resolved.
    ready: bool,
}

#[tauri::command]
fn list_pending_entries(state: State<'_, AppState>) -> Result<Vec<PendingQueueRowView>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let rows = store::list_pending_rows(&history_path_for_key(
        &state.histories_dir,
        &history.active_history,
    ))?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let local_id = store::temp_entry_id(row.seq);
            let entry = history.find(&local_id);
            let ready = match entry {
                // Same rule as the hash-resume list: any unresolved source
                // file means the payload is not final yet.
                Some(entry) if entry.kind == "files" => !entry
                    .sources
                    .files
                    .iter()
                    .any(|source| source.file_id.is_none()),
                Some(_) => true,
                None => false,
            };
            let extra = serde_json::from_str(&row.extra).unwrap_or_else(|_| {
                serde_json::json!({ "html": null, "rtf": null, "fileInfo": null, "imageInfo": null })
            });
            PendingQueueRowView {
                seq: row.seq,
                kind: row.kind,
                content: row.content,
                extra,
                created_at: row.created_at,
                exists: entry.is_some(),
                ready,
                local_id,
            }
        })
        .collect())
}

#[tauri::command(rename_all = "camelCase")]
fn acknowledge_pending_entry(state: State<'_, AppState>, seq: i64) -> Result<(), String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    store::acknowledge_pending_entry(
        &history_path_for_key(&state.histories_dir, &history.active_history),
        seq,
    )?;
    Ok(())
}

#[tauri::command]
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
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

#[tauri::command]
#[cfg(any(target_os = "android", target_os = "ios"))]
fn open_paste(_app: AppHandle) -> Result<(), String> {
    Err("移动端不支持全局快速粘贴窗口".to_string())
}

#[tauri::command]
fn start_window_drag(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    if window.label() == "paste" {
        let mut guard = state
            .paste_drag_focus_guard
            .lock()
            .map_err(|error| error.to_string())?;
        *guard = Some(Instant::now() + Duration::from_secs(5));
    }
    #[cfg(not(target_os = "windows"))]
    let _ = &state;

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    if let Err(error) = window.start_dragging() {
        #[cfg(target_os = "windows")]
        if let Ok(mut guard) = state.paste_drag_focus_guard.lock() {
            *guard = None;
        }
        return Err(error.to_string());
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let _ = &window;
    Ok(())
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn show_main_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
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

#[tauri::command(rename_all = "camelCase")]
fn show_toast(app: AppHandle, message: String, tone: String) -> Result<(), String> {
    let message = message.trim();
    if message.is_empty() {
        return Ok(());
    }
    let payload = ToastPayload {
        message: message.to_string(),
        tone: match tone.as_str() {
            "success" | "error" | "info" => tone,
            _ => "info".to_string(),
        },
    };
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?;
    let main_is_visible =
        main.is_visible().unwrap_or(false) && !main.is_minimized().unwrap_or(false);
    if main_is_visible {
        return main
            .emit("cliproam://toast", payload)
            .map_err(|error| error.to_string());
    }

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    {
        let window = app
            .get_webview_window("toast")
            .ok_or_else(|| "toast window is unavailable".to_string())?;
        position_toast_window(&app, &window)?;
        window
            .emit("cliproam://toast", payload)
            .map_err(|error| error.to_string())?;
        window.show().map_err(|error| error.to_string())?;
        return Ok(());
    }

    #[cfg(any(target_os = "android", target_os = "ios"))]
    main.emit("cliproam://toast", payload)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn hide_toast(app: AppHandle) -> Result<(), String> {
    let Some(window) = app.get_webview_window("toast") else {
        return Ok(());
    };
    window.hide().map_err(|error| error.to_string())
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn position_toast_window(app: &AppHandle, window: &tauri::WebviewWindow) -> Result<(), String> {
    let window_size = window.outer_size().map_err(|error| error.to_string())?;
    let scale_factor = window.scale_factor().map_err(|error| error.to_string())?;
    let tray_rect = app
        .tray_by_id("main")
        .ok_or_else(|| "tray icon is unavailable".to_string())?
        .rect()
        .map_err(|error| error.to_string())?;

    let (tray_position, tray_size, monitor) = if let Some(rect) = tray_rect {
        let position = match rect.position {
            Position::Physical(position) => position.cast::<i32>(),
            Position::Logical(position) => position.to_physical::<i32>(scale_factor),
        };
        let size = match rect.size {
            Size::Physical(size) => size.cast::<u32>(),
            Size::Logical(size) => size.to_physical::<u32>(scale_factor),
        };
        let monitor = window
            .monitor_from_point(
                f64::from(position.x) + f64::from(size.width) / 2.0,
                f64::from(position.y) + f64::from(size.height) / 2.0,
            )
            .map_err(|error| error.to_string())?
            .or(window.primary_monitor().map_err(|error| error.to_string())?)
            .ok_or_else(|| "monitor is unavailable".to_string())?;
        (position, size, monitor)
    } else {
        // Linux tray implementations do not expose icon bounds. Anchor the
        // toast to the primary work area's bottom-right corner instead.
        let monitor = window
            .primary_monitor()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "monitor is unavailable".to_string())?;
        let work_area = monitor.work_area();
        (
            PhysicalPosition::new(
                work_area.position.x + work_area.size.width as i32 - 24,
                work_area.position.y + work_area.size.height as i32,
            ),
            tauri::PhysicalSize::new(24, 24),
            monitor,
        )
    };
    let work_area = monitor.work_area();
    let position = calculate_toast_position(
        tray_position.x,
        tray_position.y,
        tray_size.width,
        tray_size.height,
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

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
#[allow(clippy::too_many_arguments)]
fn calculate_toast_position(
    tray_x: i32,
    tray_y: i32,
    tray_width: u32,
    tray_height: u32,
    work_x: i32,
    work_y: i32,
    work_width: u32,
    work_height: u32,
    window_width: u32,
    window_height: u32,
) -> PhysicalPosition<i32> {
    const GAP: i32 = 8;
    const MARGIN: i32 = 8;
    const EDGE_TOLERANCE: i32 = 4;
    let tray_width = tray_width as i32;
    let tray_height = tray_height as i32;
    let window_width = window_width as i32;
    let window_height = window_height as i32;
    let work_right = work_x + work_width as i32;
    let work_bottom = work_y + work_height as i32;
    let tray_right = tray_x + tray_width;
    let tray_bottom = tray_y + tray_height;
    let centered_x = tray_x + tray_width / 2 - window_width / 2;
    let centered_y = tray_y + tray_height / 2 - window_height / 2;

    let (x, y) = if tray_y >= work_bottom - EDGE_TOLERANCE {
        (centered_x, tray_y - window_height - GAP)
    } else if tray_bottom <= work_y + EDGE_TOLERANCE {
        (centered_x, tray_bottom + GAP)
    } else if tray_x >= work_right - EDGE_TOLERANCE {
        (tray_x - window_width - GAP, centered_y)
    } else if tray_right <= work_x + EDGE_TOLERANCE {
        (tray_right + GAP, centered_y)
    } else {
        (centered_x, tray_y - window_height - GAP)
    };
    let max_x = (work_right - window_width - MARGIN).max(work_x + MARGIN);
    let max_y = (work_bottom - window_height - MARGIN).max(work_y + MARGIN);
    PhysicalPosition::new(
        x.clamp(work_x + MARGIN, max_x),
        y.clamp(work_y + MARGIN, max_y),
    )
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn position_history_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    let cursor = window.cursor_position().map_err(|error| error.to_string())?;
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

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
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
        Err(format!("SendInput inserted {sent} of {} events", inputs.len()))
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn synthesize_paste() -> Result<(), String> {
    platform_clipboard::synthesize_paste()
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn synthesize_paste() -> Result<(), String> {
    Ok(())
}

enum ClipboardPayload {
    Text(RichText),
    Files(Vec<String>),
    #[cfg(target_os = "windows")]
    VirtualFiles(Box<ClipboardEntry>),
    Image(Vec<u8>),
}

#[cfg(target_os = "windows")]
fn write_clipboard_files(_app: &AppHandle, paths: &[String]) -> Result<(), String> {
    use clipboard_win::{formats::FileList, Clipboard, Setter};

    let _clipboard = Clipboard::new_attempts(10).map_err(|error| error.to_string())?;
    FileList
        .write_clipboard(paths)
        .map_err(|error| error.to_string())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_clipboard_files(app: &AppHandle, paths: &[String]) -> Result<(), String> {
    app.state::<AppState>().platform_clipboard.write_files(paths)
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn write_clipboard_files(_app: &AppHandle, _paths: &[String]) -> Result<(), String> {
    Err("当前平台暂不支持文件粘贴".to_string())
}

#[cfg(target_os = "windows")]
fn write_clipboard_image(_app: &AppHandle, image: &[u8]) -> Result<(), String> {
    use clipboard_win::{options::DoClear, raw, Clipboard};

    let bitmap = decode_image_as_bmp(image)?;
    let _clipboard = Clipboard::new_attempts(10).map_err(|error| error.to_string())?;
    // clipboard-win 5.x keeps existing formats when setting a bitmap. Clear
    // them explicitly so a text-only target cannot paste stale Unicode text
    // from the previous clipboard value when it rejects the image format.
    raw::set_bitmap_with(&bitmap, DoClear)
        .map_err(|error| error.to_string())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_clipboard_image(app: &AppHandle, image: &[u8]) -> Result<(), String> {
    app.state::<AppState>().platform_clipboard.write_image(image)
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn write_clipboard_image(_app: &AppHandle, _image: &[u8]) -> Result<(), String> {
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

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        _app
            .state::<AppState>()
            .platform_clipboard
            .write_text(&rich_text.text, rich_text.html.as_deref())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        _app.clipboard()
            .write_text(&rich_text.text)
            .map_err(|error| error.to_string())
    }
}

/// A snapshot taken under the history lock so file dialogs and disk work never
/// block the clipboard monitor.
struct EntrySnapshot {
    entry: ClipboardEntry,
    cached: HashSet<String>,
    cache_dir: PathBuf,
}

fn snapshot_entry(state: &AppState, entry_id: &str) -> Result<EntrySnapshot, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let cache_dir = active_cache_dir(state, &history);
    let entry = history
        .find(entry_id)
        .cloned()
        .ok_or_else(|| "剪贴板记录不存在".to_string())?;
    Ok(EntrySnapshot {
        entry,
        cached: history.cached_files.clone(),
        cache_dir,
    })
}

impl EntrySnapshot {
    fn resolve(&self, file_id: &str) -> Option<PathBuf> {
        readable_path(&self.cache_dir, &self.cached, &self.entry, file_id)
    }
}

fn missing_files(snapshot: &EntrySnapshot) -> Vec<MissingFile> {
    entry_contents_of(&snapshot.entry)
        .into_iter()
        .filter(|(file_id, _)| snapshot.resolve(file_id).is_none())
        .map(|(file_id, size)| MissingFile {
            file_id,
            size,
            source_device_id: snapshot.entry.source_device_id.clone(),
        })
        .collect()
}

fn refresh_snapshot_summary(state: &AppState, snapshot: &EntrySnapshot, entry_id: &str) -> Result<(), String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    refresh_entry_summary(&mut history, entry_id, &snapshot.cache_dir);
    Ok(())
}

/// Recomputes one entry's aggregates. Downloads deliberately skip this so that
/// finishing a file stays O(1); the caller refreshes once the batch is done.
#[tauri::command(rename_all = "camelCase")]
fn refresh_entry(state: State<'_, AppState>, entry_id: String) -> Result<(), String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    let cache_dir = active_cache_dir(&state, &history);
    refresh_entry_summary(&mut history, &entry_id, &cache_dir);
    Ok(())
}

/// Contents this machine cannot read yet, de-duplicated — the frontend turns
/// each one into a download.
#[tauri::command(rename_all = "camelCase")]
fn prepare_entry_files(state: State<'_, AppState>, entry_id: String) -> Result<Vec<MissingFile>, String> {
    let snapshot = snapshot_entry(&state, &entry_id)?;
    refresh_snapshot_summary(&state, &snapshot, &entry_id)?;
    Ok(missing_files(&snapshot))
}

/// Returns only the contents that must exist before this platform can start a
/// paste. The frontend does not need to know which operating system it runs on.
#[tauri::command(rename_all = "camelCase")]
fn prepare_paste_entry(state: State<'_, AppState>, entry_id: String) -> Result<Vec<MissingFile>, String> {
    let snapshot = snapshot_entry(&state, &entry_id)?;
    refresh_snapshot_summary(&state, &snapshot, &entry_id)?;
    if FilePasteStrategy::for_entry(&snapshot.entry).requires_complete_content(&snapshot.entry.kind) {
        Ok(missing_files(&snapshot))
    } else {
        Ok(Vec::new())
    }
}

#[tauri::command(rename_all = "camelCase")]
fn read_file_chunk(
    state: State<'_, AppState>,
    entry_id: String,
    file_id: String,
    offset: u64,
    length: usize,
) -> Result<String, String> {
    let (path, source_was_lost) = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        let cache_dir = active_cache_dir(&state, &history);
        let entry = history
            .find(&entry_id)
            .ok_or_else(|| "剪贴板记录不存在".to_string())?;
        let source_was_lost = local_source_was_lost(entry, &file_id);
        let path = readable_path(&cache_dir, &history.cached_files, entry, &file_id).ok_or_else(|| {
            if source_was_lost {
                "复制的源文件已删除或移动".to_string()
            } else {
                "本机文件内容不可用".to_string()
            }
        })?;
        (path, source_was_lost)
    };
    let mut file = fs::File::open(path).map_err(|error| {
        if source_was_lost {
            "复制的源文件已删除或移动".to_string()
        } else {
            error.to_string()
        }
    })?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| error.to_string())?;
    let mut bytes = vec![0; length.min(FILE_CHUNK_LIMIT)];
    let count = file.read(&mut bytes).map_err(|error| error.to_string())?;
    bytes.truncate(count);
    Ok(BASE64.encode(bytes))
}

#[tauri::command(rename_all = "camelCase")]
fn begin_file_download(
    state: State<'_, AppState>,
    transfer_id: String,
    file_id: String,
    expected_size: u64,
    save_id: Option<String>,
) -> Result<(), String> {
    let (path, target) = if let Some(save_id) = save_id {
        let mut sessions = state.save_sessions.lock().map_err(|error| error.to_string())?;
        let session = sessions
            .get_mut(&save_id)
            .ok_or_else(|| "另存为任务不存在或已结束".to_string())?;
        let expected = session
            .expected
            .get(&file_id)
            .ok_or_else(|| "文件不属于当前另存为任务".to_string())?;
        if *expected != expected_size {
            return Err("文件大小与另存为任务不一致".to_string());
        }
        if session.downloaded.contains(&file_id) || !session.in_progress.insert(file_id.clone()) {
            return Err("文件正在下载或已经下载".to_string());
        }
        let completed_path = session.staging_dir.join(&file_id);
        (
            session.staging_dir.join(format!("{file_id}.part")),
            DownloadTarget::Save {
                save_id,
                completed_path,
            },
        )
    } else {
        state.virtual_downloads.begin(&file_id);
        let history = state.history.lock().map_err(|error| error.to_string())?;
        (
            download_path(&active_cache_dir(&state, &history), &file_id)
                .ok_or_else(|| "内容标识不合法".to_string())?,
            DownloadTarget::Cache,
        )
    };
    let prepared = (|| {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::File::create(&path).map_err(|error| error.to_string())?;
        Ok::<(), String>(())
    })();
    if let Err(error) = prepared {
        clear_download_target(&state, &target, &file_id, &error);
        return Err(error);
    }
    state
        .downloads
        .lock()
        .map_err(|error| error.to_string())?
        .insert(
            transfer_id,
            DownloadState {
                path,
                file_id,
                expected_size,
                received_size: 0,
                hasher: Sha256::new(),
                target,
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
    download.hasher.update(&bytes);
    fs::OpenOptions::new()
        .append(true)
        .open(&download.path)
        .and_then(|mut file| file.write_all(&bytes))
        .map_err(|error| error.to_string())?;
    if matches!(download.target, DownloadTarget::Cache) {
        state.virtual_downloads.progress();
    }
    Ok(())
}

/// A completed cache download is registered only after digest verification.
/// Direct-save downloads stay inside the user-selected destination's staging
/// directory and never enter the application cache or its database.
#[tauri::command(rename_all = "camelCase")]
fn finish_file_download(state: State<'_, AppState>, transfer_id: String) -> Result<(), String> {
    let download = state
        .downloads
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&transfer_id)
        .ok_or_else(|| "文件下载任务不存在".to_string())?;
    if download.received_size != download.expected_size {
        let _ = fs::remove_file(&download.path);
        fail_download_target(&state, &download, "文件下载不完整");
        return Err("文件下载不完整".to_string());
    }
    if content::to_hex(&download.hasher.clone().finalize()) != download.file_id {
        let _ = fs::remove_file(&download.path);
        fail_download_target(&state, &download, "文件内容校验失败");
        return Err("文件内容校验失败".to_string());
    }

    match &download.target {
        DownloadTarget::Cache => {
            let database_path = {
                let history = state.history.lock().map_err(|error| error.to_string())?;
                history_path_for_key(&state.histories_dir, &history.active_history)
            };
            register_cached_file(&database_path, &download.file_id, download.expected_size)?;
            state
                .history
                .lock()
                .map_err(|error| error.to_string())?
                .cached_files
                .insert(download.file_id.clone());
            state.virtual_downloads.complete(&download.file_id);
        }
        DownloadTarget::Save {
            save_id,
            completed_path,
        } => {
            if completed_path.exists() {
                fs::remove_file(completed_path).map_err(|error| error.to_string())?;
            }
            fs::rename(&download.path, completed_path).map_err(|error| error.to_string())?;
            let mut sessions = state.save_sessions.lock().map_err(|error| error.to_string())?;
            let session = sessions
                .get_mut(save_id)
                .ok_or_else(|| "另存为任务不存在或已结束".to_string())?;
            session.in_progress.remove(&download.file_id);
            session.downloaded.insert(download.file_id.clone());
        }
    }
    Ok(())
}

fn fail_download_target(state: &AppState, download: &DownloadState, message: &str) {
    clear_download_target(state, &download.target, &download.file_id, message);
}

fn clear_download_target(state: &AppState, target: &DownloadTarget, file_id: &str, message: &str) {
    match target {
        DownloadTarget::Cache => state.virtual_downloads.fail(file_id, message.to_string()),
        DownloadTarget::Save { save_id, .. } => {
            if let Ok(mut sessions) = state.save_sessions.lock() {
                if let Some(session) = sessions.get_mut(save_id) {
                    session.in_progress.remove(file_id);
                }
            }
        }
    }
}

#[tauri::command(rename_all = "camelCase")]
fn cancel_file_download(
    state: State<'_, AppState>,
    transfer_id: String,
    reason: Option<String>,
) -> Result<(), String> {
    let download = state
        .downloads
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&transfer_id);
    if let Some(download) = download {
        let _ = fs::remove_file(&download.path);
        fail_download_target(
            &state,
            &download,
            &reason.unwrap_or_else(|| "文件下载已取消".to_string()),
        );
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
fn fail_virtual_file_request(
    state: State<'_, AppState>,
    file_id: String,
    message: String,
) -> Result<(), String> {
    state.virtual_downloads.fail(&file_id, message);
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn prepare_save_entry(
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<Option<SavePreparation>, String> {
    let snapshot = snapshot_entry(&state, &entry_id)?;
    let file_info = snapshot
        .entry
        .file_info
        .as_ref()
        .ok_or_else(|| "该记录不包含可另存的文件".to_string())?;
    if file_info.is_empty() {
        return Err("该记录不包含可另存的文件".to_string());
    }

    let single_file = file_info.len() == 1
        && matches!(file_info.values().next(), Some(TreeNode::File { .. }));
    let destination = if single_file {
        let name = file_info.keys().next().expect("count is one");
        let Some(destination) = rfd::FileDialog::new()
            .set_file_name(name)
            .save_file()
        else {
            return Ok(None);
        };
        destination
    } else {
        let Some(destination) = rfd::FileDialog::new().pick_folder() else {
            return Ok(None);
        };
        destination
    };

    let save_id = uuid::Uuid::new_v4().to_string();
    let staging_parent = if single_file {
        destination
            .parent()
            .ok_or_else(|| "无法确定目标目录".to_string())?
            .to_path_buf()
    } else {
        destination.clone()
    };
    let staging_dir = staging_parent.join(format!(".cliproam-save-{save_id}"));
    fs::create_dir_all(&staging_dir).map_err(|error| format!("无法准备目标目录：{error}"))?;

    let missing = missing_files(&snapshot);
    let expected = missing
        .iter()
        .map(|file| (file.file_id.clone(), file.size))
        .collect();
    state
        .save_sessions
        .lock()
        .map_err(|error| error.to_string())?
        .insert(
            save_id.clone(),
            SaveSession {
                entry_id,
                destination,
                staging_dir,
                single_file,
                expected,
                in_progress: HashSet::new(),
                downloaded: HashSet::new(),
            },
        );
    Ok(Some(SavePreparation { save_id, missing }))
}

#[tauri::command(rename_all = "camelCase")]
#[cfg(any(target_os = "android", target_os = "ios"))]
fn prepare_save_entry(
    _state: State<'_, AppState>,
    _entry_id: String,
) -> Result<Option<SavePreparation>, String> {
    Err("移动端文件已保存在应用缓存中，请使用系统分享或文件导出入口".to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn cancel_save_entry(state: State<'_, AppState>, save_id: String) -> Result<(), String> {
    let session = state
        .save_sessions
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&save_id);
    if let Some(session) = session {
        let mut downloads = state.downloads.lock().map_err(|error| error.to_string())?;
        let transfer_ids = downloads
            .iter()
            .filter_map(|(transfer_id, download)| match &download.target {
                DownloadTarget::Save { save_id: target_id, .. } if target_id == &save_id => {
                    Some(transfer_id.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for transfer_id in transfer_ids {
            if let Some(download) = downloads.remove(&transfer_id) {
                let _ = fs::remove_file(download.path);
            }
        }
        drop(downloads);
        let _ = fs::remove_dir_all(session.staging_dir);
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
fn finish_save_entry(state: State<'_, AppState>, save_id: String) -> Result<usize, String> {
    let session = state
        .save_sessions
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&save_id)
        .ok_or_else(|| "另存为任务不存在或已结束".to_string())?;
    let result = (|| {
        if !session.in_progress.is_empty() || session.downloaded.len() != session.expected.len() {
            return Err("另存为所需文件尚未下载完成".to_string());
        }
        let snapshot = snapshot_entry(&state, &session.entry_id)?;
        let file_info = snapshot
            .entry
            .file_info
            .as_ref()
            .ok_or_else(|| "该记录不包含可另存的文件".to_string())?;

        let mut resolved = HashMap::<String, PathBuf>::new();
        for (file_id, _) in tree_contents(file_info) {
            if resolved.contains_key(&file_id) {
                continue;
            }
            if session.downloaded.contains(&file_id) {
                resolved.insert(file_id.clone(), session.staging_dir.join(&file_id));
            } else {
                let source = snapshot
                    .resolve(&file_id)
                    .ok_or_else(|| format!("文件内容不可用：{file_id}"))?;
                resolved.insert(file_id.clone(), source);
            }
        }

        if session.single_file {
            let (name, node) = file_info
                .iter()
                .next()
                .ok_or_else(|| "该记录不包含可另存的文件".to_string())?;
            let TreeNode::File { f, .. } = node else {
                return Err("该记录不包含可另存的文件".to_string());
            };
            let source = resolved
                .get(f)
                .ok_or_else(|| format!("文件内容不可用：{name}"))?;
            if fs::canonicalize(source).ok() == fs::canonicalize(&session.destination).ok() {
                return Ok(0);
            }
            fs::copy(source, &session.destination).map_err(|error| format!("无法保存文件：{error}"))?;
            Ok(1)
        } else {
            // Real copies: the user owns the destination, and a hard link would
            // let a later edit reach back into the source or cache.
            rebuild_tree(&session.destination, file_info, &|file_id| resolved.get(file_id).cloned(), false)
        }
    })();
    let _ = fs::remove_dir_all(&session.staging_dir);
    result
}

/// Writes a live clipboard activation received from another device without
/// synthesizing Paste. File-list entries are deliberately excluded: they stay
/// in history until the user explicitly chooses where to paste or save them.
#[tauri::command(rename_all = "camelCase")]
fn activate_remote_entry(
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<(), String> {
    let snapshot = snapshot_entry(&state, &entry_id)?;
    let payload = match snapshot.entry.kind.as_str() {
        "files" => return Err("文件和文件夹不会自动写入漫游剪贴板".to_string()),
        "image" => {
            let file_id = snapshot
                .entry
                .image_info
                .as_ref()
                .map(|image| image.file_id.clone())
                .ok_or_else(|| "图片内容不可用".to_string())?;
            let path = snapshot
                .resolve(&file_id)
                .ok_or_else(|| "图片内容不可用".to_string())?;
            ClipboardPayload::Image(fs::read(path).map_err(|error| error.to_string())?)
        }
        _ => ClipboardPayload::Text(RichText {
            text: snapshot.entry.content.clone(),
            html: snapshot.entry.html.clone(),
            rtf: snapshot.entry.rtf.clone(),
        }),
    };

    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        match &payload {
            ClipboardPayload::Image(image) => {
                history.last_image_signature = image_signature(image);
                history.last_clipboard.clear();
                history.last_file_signature.clear();
            }
            ClipboardPayload::Text(rich_text) => {
                history.last_clipboard = rich_text_signature(rich_text);
                history.last_file_signature.clear();
                history.last_image_signature.clear();
            }
            ClipboardPayload::Files(_) => unreachable!("file activations are rejected above"),
            #[cfg(target_os = "windows")]
            ClipboardPayload::VirtualFiles(_) => unreachable!("file activations are rejected above"),
        }
        save_active_history(&state, &history)?;
    }

    match payload {
        ClipboardPayload::Text(rich_text) => write_clipboard_text(&app, &rich_text),
        ClipboardPayload::Image(image) => write_clipboard_image(&app, &image),
        ClipboardPayload::Files(_) => unreachable!("file activations are rejected above"),
        #[cfg(target_os = "windows")]
        ClipboardPayload::VirtualFiles(_) => unreachable!("file activations are rejected above"),
    }
}

fn apply_clipboard_entry(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
    synthesize: bool,
) -> Result<(), String> {
    let snapshot = snapshot_entry(&state, &entry_id)?;
    let payload = match snapshot.entry.kind.as_str() {
        "files" => {
            let file_info = snapshot
                .entry
                .file_info
                .as_ref()
                .ok_or_else(|| "该记录不包含文件".to_string())?;
            let roots = &snapshot.entry.sources.roots;
            // Copying and pasting on the same machine should not duplicate a
            // single byte, so the original paths are reused when still intact.
            let intact = !roots.is_empty()
                && roots.len() == file_info.len()
                && roots.iter().all(|path| Path::new(path).exists());
            if intact {
                ClipboardPayload::Files(roots.clone())
            } else {
                match FilePasteStrategy::for_entry(&snapshot.entry) {
                    FilePasteStrategy::VirtualStream => {
                        #[cfg(target_os = "windows")]
                        {
                            ClipboardPayload::VirtualFiles(Box::new(snapshot.entry.clone()))
                        }
                        #[cfg(not(target_os = "windows"))]
                        unreachable!("virtual file paste is only available on Windows")
                    }
                    FilePasteStrategy::MaterializedPaths => {
                        let view = snapshot
                            .cache_dir
                            .join("views")
                            .join(safe_file_name(&snapshot.entry.id));
                        let _ = fs::remove_dir_all(&view);
                        rebuild_tree(&view, file_info, &|file_id| snapshot.resolve(file_id), true)?;
                        let paths = file_info
                            .keys()
                            .map(|root| view.join(root).display().to_string())
                            .collect();
                        ClipboardPayload::Files(paths)
                    }
                }
            }
        }
        "image" => {
            let file_id = snapshot
                .entry
                .image_info
                .as_ref()
                .map(|image| image.file_id.clone())
                .ok_or_else(|| "图片内容不可用".to_string())?;
            let path = snapshot
                .resolve(&file_id)
                .ok_or_else(|| "图片内容不可用".to_string())?;
            ClipboardPayload::Image(fs::read(path).map_err(|error| error.to_string())?)
        }
        _ => ClipboardPayload::Text(RichText {
            text: snapshot.entry.content.clone(),
            html: snapshot.entry.html.clone(),
            rtf: snapshot.entry.rtf.clone(),
        }),
    };

    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        match &payload {
            ClipboardPayload::Files(paths) => {
                let paths = paths.iter().map(PathBuf::from).collect::<Vec<_>>();
                history.last_file_signature = file_signature(&paths);
                history.last_clipboard.clear();
                history.last_image_signature.clear();
            }
            #[cfg(target_os = "windows")]
            ClipboardPayload::VirtualFiles(_) => {
                history.last_file_signature.clear();
                history.last_clipboard.clear();
                history.last_image_signature.clear();
            }
            ClipboardPayload::Image(image) => {
                history.last_image_signature = image_signature(image);
                history.last_clipboard.clear();
                history.last_file_signature.clear();
            }
            ClipboardPayload::Text(rich_text) => {
                history.last_clipboard = rich_text_signature(rich_text);
                history.last_file_signature.clear();
                history.last_image_signature.clear();
            }
        }
        save_active_history(&state, &history)?;
    }

    match payload {
        ClipboardPayload::Text(rich_text) => write_clipboard_text(&app, &rich_text)?,
        ClipboardPayload::Files(paths) => write_clipboard_files(&app, &paths)?,
        #[cfg(target_os = "windows")]
        ClipboardPayload::VirtualFiles(entry) => {
            virtual_files::set_clipboard(&app, window.label(), *entry)?
        }
        ClipboardPayload::Image(image) => write_clipboard_image(&app, &image)?,
    }

    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        // Mobile operating systems do not let a normal app inject a paste into
        // another app. Keep ClipRoam visible and treat activation as Copy.
        let _ = window;
        let _ = synthesize;
        return Ok(());
    }

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    {
        if !synthesize {
            return Ok(());
        }
        window.hide().map_err(|error| error.to_string())?;
        thread::sleep(Duration::from_millis(90));
        if let Err(error) = synthesize_paste() {
            // The clipboard content is still valid, but the user needs to see why
            // automatic delivery failed (for example missing Linux helpers or
            // macOS Accessibility permission).
            let _ = window.show();
            return Err(error);
        }
        Ok(())
    }
}

#[tauri::command(rename_all = "camelCase")]
fn copy_entry(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<(), String> {
    apply_clipboard_entry(window, app, state, entry_id, false)
}

#[tauri::command(rename_all = "camelCase")]
fn paste_entry(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<(), String> {
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    if window.label() != "paste" {
        return Err("只有快捷粘贴窗口可以执行自动粘贴".to_string());
    }

    apply_clipboard_entry(window, app, state, entry_id, true)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_clipboard_manager::init());
    #[cfg(target_os = "android")]
    let builder = builder.plugin(tauri_plugin_cliproam_share_receiver::init());
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    let builder = builder
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_process::init());
    let builder = tauri_updater_kit::attach_updater(builder);

    let builder = builder
        .setup(|app| {
            #[cfg(target_os = "windows")]
            virtual_files::initialize()?;
            #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
            let window_configs = app.config().app.windows.clone();
            let app_data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
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
            let (sender, receiver) = mpsc::channel::<String>();
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            let platform_clipboard = platform_clipboard::PlatformClipboard::new()?;
            app.manage(AppState {
                history: Mutex::new(history),
                histories_dir,
                sync_config: Mutex::new(sync_config),
                sync_config_path,
                downloads: Mutex::new(HashMap::new()),
                save_sessions: Mutex::new(HashMap::new()),
                virtual_downloads: VirtualDownloads::default(),
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                platform_clipboard,
                hash_queue: Mutex::new(sender),
                share_import: Mutex::new(()),
                #[cfg(target_os = "windows")]
                paste_drag_focus_guard: Mutex::new(None),
            });

            // Desktop windows use `create: false`, so create them after managed
            // state exists. Android already creates its main webview before
            // this hook and rebuilding it would fail with a duplicate label.
            #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
            for window_config in window_configs {
                WebviewWindowBuilder::from_config(app.handle(), &window_config)?.build()?;
            }
            #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
            {
                setup_tray(app.handle())?;
                if let Some(window) = app.get_webview_window("toast") {
                    let _ = window.set_ignore_cursor_events(true);
                }
            }
            start_hash_worker(app.handle().clone(), receiver);
            #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
            start_clipboard_monitor(app.handle().clone());

            // Hashes that were still pending when the app last closed are
            // persisted, so they simply resume.
            let handle = app.handle().clone();
            thread::spawn(move || {
                let state = handle.state::<AppState>();
                let pending = match state.history.lock() {
                    Ok(mut history) => {
                        let _ = collect_local_garbage(&state.histories_dir, &mut history);
                        pending_entry_ids(&history)
                    }
                    Err(_) => Vec::new(),
                };
                for entry_id in pending {
                    queue_hashing(&state, &entry_id);
                }
            });
            Ok(())
        });

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    let builder = builder.on_window_event(|window, event| match event {
        tauri::WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = window.hide();
        }
        tauri::WindowEvent::Focused(true) if window.label() == "paste" => {
            #[cfg(target_os = "windows")]
            if let Ok(mut guard) = window.state::<AppState>().paste_drag_focus_guard.lock() {
                *guard = None;
            }
        }
        tauri::WindowEvent::Focused(false) if window.label() == "paste" => {
            let app = window.app_handle().clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(100));
                let Some(window) = app.get_webview_window("paste") else {
                    return;
                };
                #[cfg(target_os = "windows")]
                {
                    let state = window.state::<AppState>();
                    let ignore_drag_focus_loss = state
                        .paste_drag_focus_guard
                        .lock()
                        .map(|mut guard| match *guard {
                            Some(deadline) if Instant::now() <= deadline => true,
                            Some(_) => {
                                *guard = None;
                                false
                            }
                            None => false,
                        })
                        .unwrap_or(false);
                    if ignore_drag_focus_loss {
                        return;
                    }
                }
                if matches!(window.is_focused(), Ok(false)) {
                    let _ = window.hide();
                }
            });
        }
        _ => {}
    });

    builder
        .invoke_handler(tauri::generate_handler![
            get_platform_capabilities,
            capture_current_clipboard_text,
            consume_mobile_shares,
            list_entries,
            get_entry,
            list_entry_files,
            filter_unknown_file_ids,
            get_device,
            configure_device,
            get_sync_config,
            open_app_data_dir,
            save_sync_config,
            upsert_remote_entry,
            upsert_remote_entries,
            apply_published_entry,
            mark_files_uploaded,
            mark_file_available,
            delete_entry,
            clear_history,
            remove_remote_entry,
            list_pending_deletions,
            acknowledge_entry_deletion,
            list_pending_entries,
            acknowledge_pending_entry,
            open_paste,
            start_window_drag,
            hide_paste,
            hide_main,
            show_toast,
            hide_toast,
            refresh_entry,
            prepare_entry_files,
            prepare_paste_entry,
            prepare_save_entry,
            read_file_chunk,
            begin_file_download,
            append_file_download,
            finish_file_download,
            cancel_file_download,
            cancel_save_entry,
            finish_save_entry,
            fail_virtual_file_request,
            activate_remote_entry,
            copy_entry,
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

    #[test]
    fn screenshot_webp_round_trip_preserves_pixels() {
        let source = RgbaImage::from_fn(16, 12, |x, y| {
            Rgba([(x * 13) as u8, (y * 19) as u8, ((x + y) * 7) as u8, 255])
        });
        let mut bmp = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(source.clone())
            .write_to(&mut bmp, ImageFormat::Bmp)
            .unwrap();
        let bmp = bmp.into_inner();

        let (webp, width, height, thumbnail) = encode_image_as_webp(&bmp).unwrap();
        assert_eq!((width, height), (16, 12));
        assert!(thumbnail.is_some());
        assert_eq!(image::guess_format(&webp).unwrap(), ImageFormat::WebP);

        let restored_bmp = decode_image_as_bmp(&webp).unwrap();
        let restored = image::load_from_memory_with_format(&restored_bmp, ImageFormat::Bmp)
            .unwrap()
            .to_rgba8();
        assert_eq!(restored, source);
        assert_eq!(image_signature(&bmp), image_signature(&webp));
    }

    #[test]
    fn macos_html_transport_wrapper_keeps_the_same_signature() {
        let fragment = RichText {
            text: "hello".to_string(),
            html: Some("<b>hello</b>".to_string()),
            rtf: None,
        };
        let wrapped = RichText {
            text: fragment.text.clone(),
            html: Some(format!(
                "<html><head><meta http-equiv=\"content-type\" content=\"text/html; charset=utf-8\"></head><body>{}</body></html>",
                fragment.html.as_deref().unwrap()
            )),
            rtf: None,
        };
        assert_eq!(rich_text_signature(&fragment), rich_text_signature(&wrapped));
    }

    #[test]
    fn paste_strategy_owns_platform_materialization_policy() {
        let virtual_stream = FilePasteStrategy::VirtualStream;
        let materialized = FilePasteStrategy::MaterializedPaths;

        assert!(!virtual_stream.requires_complete_content("files"));
        assert!(virtual_stream.requires_complete_content("image"));
        assert!(materialized.requires_complete_content("files"));
        assert!(materialized.requires_complete_content("image"));
        assert!(!materialized.requires_complete_content("text"));
    }

    #[test]
    fn paste_window_position_stays_inside_the_work_area() {
        let position = calculate_history_position(1900, 1050, 0, 0, 1920, 1080, 420, 560);
        assert!(position.x + 420 <= 1920);
        assert!(position.y + 560 <= 1080);
    }

    #[test]
    fn toast_appears_above_a_bottom_tray() {
        let position =
            calculate_toast_position(1850, 1040, 32, 32, 0, 0, 1920, 1040, 380, 88);
        assert_eq!(position.y, 944);
        assert!(position.x >= 8);
        assert!(position.x + 380 <= 1912);
    }

    #[test]
    fn toast_appears_beside_a_right_tray() {
        let position =
            calculate_toast_position(1920, 850, 40, 32, 0, 0, 1920, 1080, 380, 88);
        assert_eq!(position.x, 1532);
        assert!(position.y >= 8);
        assert!(position.y + 88 <= 1072);
    }
}
