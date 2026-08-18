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
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{mpsc, Condvar, Mutex},
    thread,
    time::Duration,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, State,
};
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
use tauri_plugin_clipboard_manager::ClipboardExt;

use content::{
    collect_tree, create_pack, describe_roots, download_path, file_entry_signature, file_signature,
    hash_bytes, hash_file, local_source_was_lost, new_tree, preserve_local_sources, readable_path,
    rebuild_entry_files, rebuild_tree, transfer_file_id, unpack_pack, unpacked_file_path, upload_image_path,
    ClipboardEntry, ClipboardFile, ClipboardTreeFile, ClipboardTreeRoot, LocalSources,
};
use store::{
    cache_dir_for, cached_hash, collect_local_garbage, default_active_history, history_path_for_key,
    load_history, open_history_database, refresh_entry_summary, register_cached_file, remember_hash,
    retain_single_history, save_history, trim_history, write_entry_contents, HistoryData, LOCAL_HISTORY_KEY,
};

const TRAY_SHOW_MAIN: &str = "show-main";
const TRAY_QUIT: &str = "quit";
const FILE_CHUNK_LIMIT: usize = 128 * 1024;
const THUMBNAIL_MAX_EDGE: u32 = 64;
const THUMBNAIL_MAX_BYTES: usize = 72 * 1024;
/// How many freshly hashed paths are folded into the entry before the UI is
/// told about the progress.
const HASH_PROGRESS_BATCH: usize = 32;
/// New large trees keep big contents independent while grouping small contents
/// into stable hash-prefix buckets. Eight MiB stays below the default 10 MiB
/// automatic upload threshold.
const PACK_TREE_FILE_THRESHOLD: usize = 200;
const PACK_MIN_CONTENTS: usize = 128;
const PACK_MAX_CONTENT_SIZE: u64 = 1024 * 1024;
const PACK_TARGET_SIZE: u64 = 8 * 1024 * 1024;

#[derive(Clone)]
struct PackCandidate {
    file_id: String,
    source: PathBuf,
    size: u64,
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

struct AppState {
    history: Mutex<HistoryData>,
    histories_dir: PathBuf,
    sync_config: Mutex<Option<SyncConfig>>,
    sync_config_path: PathBuf,
    downloads: Mutex<HashMap<String, DownloadState>>,
    virtual_downloads: VirtualDownloads,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    platform_clipboard: platform_clipboard::PlatformClipboard,
    /// `Sender` is not `Sync`, so managed state has to guard it.
    hash_queue: Mutex<mpsc::Sender<String>>,
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
    /// Pack extraction has a shared destination per pack id.
    unpack: Mutex<()>,
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
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MissingFile {
    file_id: String,
    size: u64,
    source_device_id: String,
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
    let mut lightweight = ClipboardEntry {
        tree: None,
        files: Vec::new(),
        sources: LocalSources::default(),
        ..entry.clone()
    };
    if lightweight.kind == "files" {
        if let Some(tree) = &entry.tree {
            lightweight.content = describe_roots(&tree.roots);
        }
    }
    lightweight
}

#[cfg(not(target_os = "windows"))]
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

/// Entry identity is owned by the capturing device. The NUL separator keeps
/// the two variable-length inputs unambiguous while preserving the requested
/// `sha256(content + deviceId)` identity model.
fn entry_id(content: &str, device_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hasher.update([0]);
    hasher.update(device_id.as_bytes());
    content::to_hex(&hasher.finalize())
}

fn entry_id_for_files(files: &[ClipboardFile], device_id: &str, fallback: &str) -> String {
    let mut file_ids = files
        .iter()
        .map(|file| file.file_id.as_str())
        .filter(|file_id| !file_id.is_empty())
        .collect::<Vec<_>>();
    file_ids.sort_unstable();
    file_ids.dedup();
    let identity = if file_ids.is_empty() {
        fallback.to_string()
    } else {
        file_ids.join("\n")
    };
    entry_id(&identity, device_id)
}

fn new_entry(kind: &str, content: String, device_id: String) -> ClipboardEntry {
    ClipboardEntry {
        id: entry_id(&content, &device_id),
        kind: kind.to_string(),
        content,
        html: None,
        rtf: None,
        thumbnail: None,
        tree: None,
        files: Vec::new(),
        source_device_id: device_id,
        created_at: Utc::now().to_rfc3339(),
        pinned: false,
        summary: Default::default(),
        sources: LocalSources::default(),
    }
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
        let mut entry = new_entry("text", rich_text.text, device_id);
        entry.html = rich_text.html;
        entry.rtf = rich_text.rtf;
        let entries = history.active_entries_mut();
        entries.retain(|item| item.content != entry.content);
        entries.insert(0, entry.clone());
        trim_history(entries);
        save_active_history(&state, &history)?;
        entry
    };
    // Text has no contents to hash, so it is publishable the moment it lands —
    // the frontend only ever publishes on `entry-ready`.
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
        let device_id = history.device_id.clone();
        let created_at = Utc::now().to_rfc3339();
        let content = describe_roots(&collected.tree.roots);
        let entries = history.active_entries_mut();
        let entry_id = match entries
            .iter()
            .position(|item| item.kind == "files" && file_entry_signature(item) == signature)
        {
            Some(index) => {
                let mut existing = entries.remove(index);
                existing.created_at = created_at;
                let entry_id = existing.id.clone();
                entries.insert(0, existing);
                entry_id
            }
            None => {
                let mut entry = new_entry("files", content, device_id);
                entry.created_at = created_at;
                entry.tree = Some(collected.tree);
                entry.sources = collected.sources;
                rebuild_entry_files(&mut entry);
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
        let name = format!("{}.webp", &file_id[..16]);
        let mut entry = new_entry("image", format!("截图（{width} × {height}）"), device_id.clone());
        entry.id = entry_id(&file_id, &device_id);
        entry.thumbnail = thumbnail;
        let mut tree = new_tree();
        tree.roots.push(ClipboardTreeRoot {
            name: name.clone(),
            kind: "file".to_string(),
        });
        tree.files.push(ClipboardTreeFile {
            p: name,
            f: file_id.clone(),
            s: Some(webp.len() as u64),
            b: None,
        });
        entry.tree = Some(tree);
        entry.files = vec![ClipboardFile {
            file_id,
            size: webp.len() as u64,
            available: false,
        }];
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

fn start_clipboard_monitor(app: AppHandle) {
    thread::spawn(move || loop {
        if let Some(paths) = read_clipboard_files(&app).filter(|paths| !paths.is_empty()) {
            let _ = capture_files(&app, paths);
        } else if let Some(rich_text) = read_clipboard_text(&app) {
            let _ = capture_text(&app, rich_text);
        } else if let Some(image) = read_clipboard_image(&app) {
            let _ = capture_image(&app, image);
        }
        thread::sleep(Duration::from_millis(350));
    });
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
    let mut current_entry_id = entry_id.to_string();
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
            let Some(updated_entry_id) = apply_hashes(app, &current_entry_id, &batch, false)? else {
                return Ok(());
            };
            current_entry_id = updated_entry_id;
            batch.clear();
        }
    }
    let Some(final_entry_id) = apply_hashes(app, &current_entry_id, &batch, true)? else {
        return Ok(());
    };
    // Packing is a transport optimization. If a source changes during the
    // extra read, keep the already valid per-file entry publishable.
    if let Err(error) = pack_small_contents(app, &final_entry_id) {
        eprintln!("ClipRoam: 打包 {final_entry_id} 的小文件失败，改用单文件传输：{error}");
    }
    app.emit("cliproam://entry-ready", final_entry_id)
        .map_err(|error| error.to_string())
}

fn split_pack_candidates(candidates: Vec<PackCandidate>, depth: usize) -> Vec<Vec<PackCandidate>> {
    let encoded_size = candidates
        .iter()
        .map(|candidate| candidate.size + 64 + 8)
        .sum::<u64>();
    if encoded_size <= PACK_TARGET_SIZE || candidates.len() <= 1 || depth >= 64 {
        return vec![candidates];
    }
    let mut buckets = vec![Vec::new(); 16];
    for candidate in candidates {
        let nibble = match candidate.file_id.as_bytes()[depth] {
            b'0'..=b'9' => candidate.file_id.as_bytes()[depth] - b'0',
            b'a'..=b'f' => candidate.file_id.as_bytes()[depth] - b'a' + 10,
            _ => 0,
        };
        buckets[nibble as usize].push(candidate);
    }
    buckets
        .into_iter()
        .filter(|bucket| !bucket.is_empty())
        .flat_map(|bucket| split_pack_candidates(bucket, depth + 1))
        .collect()
}

/// Replaces the transfer identity of many small contents with bounded packs.
/// Tree nodes retain their original hashes, so extraction can independently
/// verify every file and entry identity does not depend on packing policy.
fn pack_small_contents(app: &AppHandle, entry_id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (entry, cache_dir, database_path) = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        let entry = history
            .find(entry_id)
            .cloned()
            .ok_or_else(|| "剪贴板记录不存在".to_string())?;
        (
            entry,
            active_cache_dir(&state, &history),
            history_path_for_key(&state.histories_dir, &history.active_history),
        )
    };
    let Some(tree) = entry.tree.as_ref() else {
        return Ok(());
    };
    if tree.files.len() < PACK_TREE_FILE_THRESHOLD || tree.files.iter().any(|node| node.b.is_some()) {
        return Ok(());
    }

    let mut candidates = entry
        .sources
        .files
        .iter()
        .filter(|source| source.size <= PACK_MAX_CONTENT_SIZE)
        .filter_map(|source| {
            source.file_id.as_ref().map(|file_id| PackCandidate {
                file_id: file_id.clone(),
                source: PathBuf::from(&source.source),
                size: source.size,
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.file_id.cmp(&right.file_id));
    candidates.dedup_by(|left, right| left.file_id == right.file_id);
    if candidates.len() < PACK_MIN_CONTENTS {
        return Ok(());
    }

    let groups = split_pack_candidates(candidates, 0);
    let mut member_to_pack = HashMap::new();
    let mut packed_files = HashMap::new();
    for group in groups {
        let temporary = cache_dir
            .join("download")
            .join(format!(".pack-{}", uuid::Uuid::new_v4()));
        let contents = group
            .iter()
            .map(|candidate| (candidate.file_id.clone(), candidate.source.clone()))
            .collect::<Vec<_>>();
        let (pack_id, pack_size) = create_pack(&temporary, &contents)?;
        let target = download_path(&cache_dir, &pack_id).ok_or_else(|| "文件包标识不合法".to_string())?;
        if target.is_file() {
            fs::remove_file(&temporary).map_err(|error| error.to_string())?;
        } else {
            fs::rename(&temporary, &target).map_err(|error| error.to_string())?;
        }
        register_cached_file(&database_path, &pack_id, pack_size)?;
        for candidate in group {
            member_to_pack.insert(candidate.file_id, pack_id.clone());
        }
        packed_files.insert(pack_id.clone(), ClipboardFile {
            file_id: pack_id,
            size: pack_size,
            available: false,
        });
    }

    let known = entry
        .files
        .iter()
        .map(|file| (file.file_id.clone(), file.clone()))
        .collect::<HashMap<_, _>>();
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    let cache_dir = active_cache_dir(&state, &history);
    let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
    let Some(current) = history.find_mut(entry_id) else {
        return Ok(());
    };
    let Some(tree) = current.tree.as_mut() else {
        return Ok(());
    };
    for node in &mut tree.files {
        node.b = member_to_pack.get(&node.f).cloned();
    }
    let mut seen = HashSet::new();
    current.files = tree
        .files
        .iter()
        .filter_map(|node| {
            let transfer_id = transfer_file_id(node);
            if !seen.insert(transfer_id.to_string()) {
                return None;
            }
            packed_files.get(transfer_id).cloned().or_else(|| known.get(transfer_id).cloned())
        })
        .collect();
    let connection = open_history_database(&history_path)?;
    write_entry_contents(&connection, current, true)?;
    for pack_id in packed_files.keys() {
        history.cached_files.insert(pack_id.clone());
    }
    refresh_entry_summary(&mut history, entry_id, &cache_dir);
    Ok(())
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
    let device_id = history.device_id.clone();
    let current_entry_index = history
        .active_entries()
        .iter()
        .position(|entry| entry.id == entry_id);
    let hashes = resolved
        .iter()
        .map(|(path, file_id)| (path.as_str(), file_id.as_deref()))
        .collect::<HashMap<_, _>>();
    let final_entry_id = {
        let Some(entry) = history.find_mut(entry_id) else {
            return Ok(None);
        };
        if let Some(tree) = entry.tree.as_mut() {
            tree.files.retain_mut(|node| match hashes.get(node.p.as_str()) {
                Some(Some(file_id)) => {
                    node.f = (*file_id).to_string();
                    true
                }
                Some(None) => false,
                None => true,
            });
        }
        entry.sources.files.retain_mut(|source| match hashes.get(source.path.as_str()) {
            Some(Some(file_id)) => {
                source.file_id = Some((*file_id).to_string());
                true
            }
            Some(None) => false,
            None => true,
        });
        rebuild_entry_files(entry);
        if persist {
            entry.id = entry_id_for_files(&entry.files, &device_id, &entry.content);
        }
        entry.id.clone()
    };
    if persist && final_entry_id != entry_id {
        let entries = history.active_entries_mut();
        if let Some(index) = entries.iter().enumerate().find_map(|(index, entry)| {
            (Some(index) != current_entry_index && entry.id == final_entry_id).then_some(index)
        })
        {
            entries.remove(index);
        }
    }
    refresh_entry_summary(&mut history, &final_entry_id, &cache_dir);
    if persist {
        save_active_history(&state, &history)?;
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
    let decoded = image::load_from_memory_with_format(image, ImageFormat::Bmp)
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

#[tauri::command]
fn supports_virtual_file_paste() -> bool {
    cfg!(target_os = "windows")
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

#[tauri::command(rename_all = "camelCase")]
fn list_entry_files(state: State<'_, AppState>, entry_id: String) -> Result<Vec<ClipboardFile>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    history
        .find(&entry_id)
        .map(|entry| entry.files.clone())
        .ok_or_else(|| "剪贴板记录不存在".to_string())
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
) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let cache_dir = active_cache_dir(&state, &history);
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
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
        // The entry row must exist before its contents can be replaced: the
        // latter has a foreign key to `entries`.
        if let Some(entry) = history.find(&entry_id) {
            let connection = open_history_database(&history_path)?;
            write_entry_contents(&connection, entry, true)?;
        }
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
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
        let uploaded = file_ids.into_iter().collect::<HashSet<_>>();
        {
            let Some(entry) = history.find_mut(&entry_id) else {
                return Ok(());
            };
            for file in entry.files.iter_mut() {
                if uploaded.contains(&file.file_id) {
                    file.available = true;
                }
            }
            let connection = open_history_database(&history_path)?;
            write_entry_contents(&connection, entry, true)?;
        }
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
        let changed_ids = {
            let entries = history.active_entries_mut();
            entries
                .iter_mut()
                .filter_map(|entry| {
                    let mut changed = false;
                    for file in entry.files.iter_mut() {
                        if file.file_id == file_id && !file.available {
                            file.available = true;
                            changed = true;
                        }
                    }
                    changed.then(|| entry.id.clone())
                })
                .collect::<HashSet<_>>()
        };
        if changed_ids.is_empty() {
            return Ok(());
        }
        let changed_entries = history
            .active_entries()
            .iter()
            .filter(|entry| changed_ids.contains(&entry.id))
            .cloned()
            .collect::<Vec<_>>();
        let connection = open_history_database(&history_path)?;
        for entry in &changed_entries {
            write_entry_contents(&connection, entry, true)?;
            refresh_entry_summary(&mut history, &entry.id, &cache_dir);
        }
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
        let changed = if let Some(entry) = history.find_mut(&entry_id) {
            entry.pinned = pinned;
            true
        } else {
            false
        };
        if changed {
            history.pending_entry_updates.insert(entry_id);
        }
        save_active_history(&state, &history)?;
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
            history.pending_deletions.insert(entry_id.clone());
            history.pending_entry_updates.remove(&entry_id);
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
            .filter(|entry| !entry.pinned)
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        history.active_entries_mut().retain(|entry| entry.pinned);
        for entry_id in deleted {
            history.pending_deletions.insert(entry_id.clone());
            history.pending_entry_updates.remove(&entry_id);
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

#[tauri::command]
fn list_pending_entry_updates(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let mut pending = history.pending_entry_updates.iter().cloned().collect::<Vec<_>>();
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

#[tauri::command(rename_all = "camelCase")]
fn acknowledge_entry_update(state: State<'_, AppState>, entry_id: String) -> Result<(), String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    if history.pending_entry_updates.remove(&entry_id) {
        save_active_history(&state, &history)?;
    }
    Ok(())
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
    use clipboard_win::{formats::Bitmap, Clipboard, Setter};

    let bitmap = decode_image_as_bmp(image)?;
    let _clipboard = Clipboard::new_attempts(10).map_err(|error| error.to_string())?;
    Bitmap
        .write_clipboard(&bitmap)
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

    fn resolve_content(&self, file_id: &str) -> Option<PathBuf> {
        if let Some(path) = self.resolve(file_id) {
            return Some(path);
        }
        let pack_id = self
            .entry
            .tree
            .as_ref()?
            .files
            .iter()
            .find(|node| node.f == file_id)?
            .b
            .as_deref()?;
        let pack_path = self.resolve(pack_id)?;
        let target = unpacked_file_path(&self.cache_dir, pack_id, file_id)?;
        let destination = target.parent()?;
        unpack_pack(&pack_path, destination).ok()?;
        target.is_file().then_some(target)
    }
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
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        refresh_entry_summary(&mut history, &entry_id, &snapshot.cache_dir);
    }
    Ok(snapshot
        .entry
        .files
        .iter()
        .filter(|file| snapshot.resolve(&file.file_id).is_none())
        .map(|file| MissingFile {
            file_id: file.file_id.clone(),
            size: file.size,
            source_device_id: snapshot.entry.source_device_id.clone(),
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
) -> Result<(), String> {
    state.virtual_downloads.begin(&file_id);
    let path = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        download_path(&active_cache_dir(&state, &history), &file_id)
            .ok_or_else(|| "内容标识不合法".to_string())?
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::File::create(&path).map_err(|error| error.to_string())?;
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
    state.virtual_downloads.progress();
    Ok(())
}

/// A completed download is retained at `files/download/<fileId>` only after
/// its digest matches the id used by history and sync.
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
        state.virtual_downloads.fail(&download.file_id, "文件下载不完整".to_string());
        return Err("文件下载不完整".to_string());
    }
    if content::to_hex(&download.hasher.finalize()) != download.file_id {
        let _ = fs::remove_file(&download.path);
        state.virtual_downloads.fail(&download.file_id, "文件内容校验失败".to_string());
        return Err("文件内容校验失败".to_string());
    }

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
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
fn cancel_file_download(
    state: State<'_, AppState>,
    transfer_id: String,
    reason: Option<String>,
) -> Result<(), String> {
    if let Some(download) = state
        .downloads
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&transfer_id)
    {
        let _ = fs::remove_file(download.path);
        state.virtual_downloads.fail(
            &download.file_id,
            reason.unwrap_or_else(|| "文件下载已取消".to_string()),
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
fn save_entry_files(state: State<'_, AppState>, entry_id: String) -> Result<usize, String> {
    let snapshot = snapshot_entry(&state, &entry_id)?;
    let tree = snapshot
        .entry
        .tree
        .as_ref()
        .ok_or_else(|| "该记录不包含可另存的文件".to_string())?;
    if tree.roots.is_empty() {
        return Err("该记录不包含可另存的文件".to_string());
    }

    if tree.roots.len() == 1 && tree.roots[0].kind == "file" {
        let node = tree.files.first().ok_or_else(|| "该记录不包含可另存的文件".to_string())?;
        let source = snapshot
            .resolve(&node.f)
            .ok_or_else(|| format!("文件内容不可用：{}", tree.roots[0].name))?;
        let Some(destination) = rfd::FileDialog::new()
            .set_file_name(&tree.roots[0].name)
            .save_file()
        else {
            return Ok(0);
        };
        if fs::canonicalize(&source).ok() == fs::canonicalize(&destination).ok() {
            return Ok(0);
        }
        fs::copy(&source, &destination).map_err(|error| format!("无法保存文件：{error}"))?;
        return Ok(1);
    }

    let Some(destination) = rfd::FileDialog::new().pick_folder() else {
        return Ok(0);
    };
    // Real copies: the user owns the destination, and a hard link would let a
    // later edit reach back into the cache.
    rebuild_tree(&destination, tree, &|file_id| snapshot.resolve_content(file_id), false)
}

#[tauri::command(rename_all = "camelCase")]
fn paste_entry(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<(), String> {
    let snapshot = snapshot_entry(&state, &entry_id)?;
    let payload = match snapshot.entry.kind.as_str() {
        "files" => {
            let tree = snapshot
                .entry
                .tree
                .as_ref()
                .ok_or_else(|| "该记录不包含文件".to_string())?;
            let roots = &snapshot.entry.sources.roots;
            // Copying and pasting on the same machine should not duplicate a
            // single byte, so the original paths are reused when still intact.
            let intact = !roots.is_empty()
                && roots.len() == tree.roots.len()
                && roots.iter().all(|path| Path::new(path).exists());
            if intact {
                ClipboardPayload::Files(roots.clone())
            } else {
                #[cfg(target_os = "windows")]
                {
                    ClipboardPayload::VirtualFiles(Box::new(snapshot.entry.clone()))
                }
                #[cfg(not(target_os = "windows"))]
                {
                let view = snapshot.cache_dir.join("views").join(safe_file_name(&snapshot.entry.id));
                let _ = fs::remove_dir_all(&view);
                rebuild_tree(&view, tree, &|file_id| snapshot.resolve_content(file_id), true)?;
                let paths = tree.roots
                    .iter()
                    .map(|root| view.join(&root.name).display().to_string())
                    .collect();
                ClipboardPayload::Files(paths)
                }
            }
        }
        "image" => {
            let file_id = snapshot
                .entry
                .files
                .first()
                .map(|file| file.file_id.clone())
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
            #[cfg(target_os = "windows")]
            virtual_files::initialize()?;
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
                virtual_downloads: VirtualDownloads::default(),
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                platform_clipboard,
                hash_queue: Mutex::new(sender),
            });
            setup_tray(app.handle())?;
            start_hash_worker(app.handle().clone(), receiver);
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
            supports_virtual_file_paste,
            get_entry,
            list_entry_files,
            get_device,
            configure_device,
            get_sync_config,
            open_app_data_dir,
            save_sync_config,
            upsert_remote_entry,
            mark_files_uploaded,
            mark_file_available,
            set_pinned,
            delete_entry,
            clear_history,
            remove_remote_entry,
            list_pending_deletions,
            list_pending_entry_updates,
            acknowledge_entry_deletion,
            acknowledge_entry_update,
            open_paste,
            hide_paste,
            hide_main,
            refresh_entry,
            prepare_entry_files,
            read_file_chunk,
            begin_file_download,
            append_file_download,
            finish_file_download,
            cancel_file_download,
            fail_virtual_file_request,
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
    fn entry_id_is_stable_per_content_and_device() {
        assert_eq!(entry_id("same", "device"), entry_id("same", "device"));
        assert_ne!(entry_id("same", "device"), entry_id("other", "device"));
        assert_ne!(entry_id("same", "device"), entry_id("same", "other-device"));
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
    fn file_entry_id_is_order_independent_and_deduplicated() {
        let first = ClipboardFile {
            file_id: hash_bytes(b"first"),
            size: 1,
            available: false,
        };
        let second = ClipboardFile {
            file_id: hash_bytes(b"second"),
            size: 2,
            available: false,
        };
        assert_eq!(
            entry_id_for_files(
                &[first.clone(), second.clone(), first.clone()],
                "device",
                "fallback",
            ),
            entry_id_for_files(&[second, first], "device", "fallback"),
        );
    }

    #[test]
    fn many_small_contents_are_split_into_bounded_stable_packs() {
        let candidates = (0..300)
            .map(|index| PackCandidate {
                file_id: hash_bytes(format!("file-{index}").as_bytes()),
                source: PathBuf::from(format!("file-{index}")),
                size: 100 * 1024,
            })
            .collect::<Vec<_>>();
        let groups = split_pack_candidates(candidates.clone(), 0);
        let repeated = split_pack_candidates(candidates, 0);

        assert!(groups.len() < 300);
        assert_eq!(groups.iter().map(Vec::len).sum::<usize>(), 300);
        assert!(groups.iter().all(|group| {
            group.iter().map(|candidate| candidate.size + 72).sum::<u64>() <= PACK_TARGET_SIZE
        }));
        assert_eq!(
            groups
                .iter()
                .map(|group| group.iter().map(|candidate| candidate.file_id.as_str()).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            repeated
                .iter()
                .map(|group| group.iter().map(|candidate| candidate.file_id.as_str()).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn paste_window_position_stays_inside_the_work_area() {
        let position = calculate_history_position(1900, 1050, 0, 0, 1920, 1080, 420, 560);
        assert!(position.x + 420 <= 1920);
        assert!(position.y + 560 <= 1080);
    }
}
