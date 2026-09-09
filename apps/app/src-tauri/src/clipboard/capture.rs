//! Clipboard capture: reading clipboard payloads and turning them into
//! history entries backed by the durable upload queue.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use image::{GenericImageView, ImageFormat};
use serde::Serialize;
use std::{
    fs,
    io::Cursor,
    path::PathBuf,
    thread,
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::content::{
    collect_tree, describe_roots, file_entry_signature, file_signature, fnv1a, hash_bytes,
    upload_image_path, ClipboardEntry, ClipboardEntryExtra, ImageInfo, LocalSources,
};
use crate::store::{
    enqueue_pending_entry, ensure_pending_entry, refresh_entry_summary, temp_entry_seq,
    trim_history, history_path_for_key,
};
use crate::AppState;

const THUMBNAIL_MAX_EDGE: u32 = 64;
const THUMBNAIL_MAX_BYTES: usize = 72 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct RichText {
    pub text: String,
    pub html: Option<String>,
    pub rtf: Option<String>,
}

pub(crate) fn safe_file_name(name: &str) -> String {
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

pub(crate) fn rich_text_signature(rich_text: &RichText) -> String {
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
    let values = [
        Some(rich_text.text.as_str()),
        html,
        rich_text.rtf.as_deref(),
    ];
    let hash = fnv1a(
        values
            .into_iter()
            .flat_map(|value| value.unwrap_or_default().bytes().chain(std::iter::once(0))),
    );
    format!("{hash:016x}")
}

pub(crate) fn image_signature(image: &[u8]) -> String {
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
    let hash = fnv1a(bytes.iter().copied());
    format!("{prefix}:{hash:016x}")
}

/// Local, pre-publish entry identity: the seq of the capture's durable queue
/// row. The server assigns the real id when the entry is first published and
/// `apply_published_entry` swaps it out, so this only has to stay stable until
/// then.
pub(crate) fn new_entry(seq: i64, kind: &str, content: String, device_id: String) -> ClipboardEntry {
    ClipboardEntry {
        id: crate::store::temp_entry_id(seq),
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
    history: &crate::store::HistoryData,
    kind: &str,
    content: &str,
    extra: &ClipboardEntryExtra,
    created_at: &str,
) -> Result<i64, String> {
    let payload = extra.json()?;
    enqueue_pending_entry(
        &history_path_for_key(&state.histories_dir, &history.active_history),
        kind,
        content,
        &payload,
        created_at,
    )
}

/// The frontend renders lists of hundreds of entries; shipping their trees
/// would mean tens of thousands of nodes per refresh.
pub(crate) fn lightweight_entry(entry: &ClipboardEntry) -> ClipboardEntry {
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

pub(crate) fn capture_text(app: &AppHandle, rich_text: RichText) -> Result<(), String> {
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
        crate::save_active_history(&state, &history)?;
        entry
    };
    // Text has no contents to hash, so it is publishable the moment it lands —
    // the frontend drains the queue whenever an entry becomes ready.
    app.emit("cliproam://entry-created", lightweight_entry(&entry))
        .map_err(|error| error.to_string())?;
    app.emit("cliproam://entry-ready", entry.id)
        .map_err(|error| error.to_string())
}

pub(crate) fn capture_files(app: &AppHandle, paths: Vec<PathBuf>) -> Result<(), String> {
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
        let cache_dir = crate::active_cache_dir(&state, &history);
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
                if let Some(seq) = temp_entry_seq(&existing.id) {
                    let payload = ClipboardEntryExtra::of(&existing).json()?;
                    if let Err(error) = ensure_pending_entry(
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
                let payload = extra.json()?;
                let seq = match enqueue_pending_entry(&history_path, "files", &content, &payload, &created_at)
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
        crate::save_active_history(&state, &history)?;
        let entry = history
            .find(&entry_id)
            .map(lightweight_entry)
            .ok_or_else(|| "剪贴板记录不存在".to_string())?;
        (entry, entry_id)
    };
    crate::clipboard::hashing::queue_hashing(&state, &entry_id);
    app.emit("cliproam://entry-created", entry)
        .map_err(|error| error.to_string())
}

pub(crate) fn capture_image(app: &AppHandle, image: Vec<u8>) -> Result<(), String> {
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
        let cache_dir = crate::active_cache_dir(&state, &history);
        let image_path = upload_image_path(&cache_dir, &file_id).ok_or_else(|| "内容标识不合法".to_string())?;
        if let Some(parent) = image_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        if !image_path.is_file() {
            fs::write(&image_path, &webp).map_err(|error| error.to_string())?;
        }
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
        let payload = extra.json()?;
        let seq = match enqueue_pending_entry(
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
        crate::save_active_history(&state, &history)?;
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

#[tauri::command]
pub(crate) fn capture_current_clipboard_text(app: AppHandle) -> Result<bool, String> {
    let Some(rich_text) = crate::platforms::read_clipboard_text(&app) else {
        return Ok(false);
    };
    capture_text(&app, rich_text)?;
    Ok(true)
}

pub(crate) fn encode_image_as_webp(image: &[u8]) -> Result<(Vec<u8>, u32, u32, Option<String>), String> {
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

pub(crate) fn decode_image_as_bmp(image: &[u8]) -> Result<Vec<u8>, String> {
    let decoded = image::load_from_memory(image).map_err(|error| error.to_string())?;
    let mut output = Cursor::new(Vec::new());
    decoded
        .write_to(&mut output, ImageFormat::Bmp)
        .map_err(|error| error.to_string())?;
    Ok(output.into_inner())
}

// ---------------------------------------------------------------------------
// 剪贴板轮询：把当前 OS 剪贴板变成历史条目。
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub(crate) fn start_clipboard_monitor(app: AppHandle) {
    thread::spawn(move || {
        // Every pass reads the full clipboard and re-decodes its contents for
        // the signatures (a bitmap can be tens of megabytes), so Windows gates
        // each pass on the clipboard sequence number (see
        // platforms::should_skip_clipboard_poll): an unchanged clipboard — a
        // screenshot sitting idle for hours, for example — is skipped without
        // even opening it.
        let mut last_clipboard_sequence = 0u32;
        loop {
            if crate::platforms::should_skip_clipboard_poll(&mut last_clipboard_sequence) {
                continue;
            }
            if let Some(paths) = crate::platforms::read_clipboard_files(&app).filter(|paths| !paths.is_empty()) {
                let _ = capture_files(&app, paths);
            } else if let Some(rich_text) = crate::platforms::read_clipboard_text(&app) {
                let _ = capture_text(&app, rich_text);
            } else if let Some(image) = crate::platforms::read_clipboard_image(&app) {
                let _ = capture_image(&app, image);
            }
            thread::sleep(Duration::from_millis(350));
        }
    });
}

// ---------------------------------------------------------------------------
// Android 分享接收：分享项先进入待处理队列，再按本地捕获一样导入历史。
// 各系统的导入实现见 platforms/<系统>；这里只负责串行化导入。
// ---------------------------------------------------------------------------

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShareImportSummary {
    shares: usize,
    texts: usize,
    images: usize,
    files: usize,
}

#[tauri::command]
pub(crate) fn consume_mobile_shares(app: AppHandle, state: State<'_, AppState>) -> Result<ShareImportSummary, String> {
    let _guard = state.share_import.lock().map_err(|error| error.to_string())?;
    crate::platforms::consume_pending_shares(&app)
}
