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
    refresh_summary, upload_image_path, ClipboardEntry, ClipboardEntryExtra, ImageInfo,
    LocalSources,
};
use crate::pending::{delete_rows_for, enqueue};
use crate::store::{
    delete_entries_by_ids, history_path_for_key, save_metadata, select_entries, upsert_entry_row,
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
        id: crate::pending::temp_entry_id(seq),
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

/// Finds the files entry to reuse for a re-copy of the same roots. A LIKE
/// prefilter over the JSON `sources` column narrows the candidates (the
/// exact signature stats the root paths, so it must not run per row), then
/// the signature decides. Newest first, like the scan it replaces.
fn find_reusable_files_entry(
    state: &AppState,
    history_path: &std::path::Path,
    paths: &[PathBuf],
    signature: &str,
) -> Result<Option<ClipboardEntry>, String> {
    // JSON escapes backslashes, so a Windows root appears doubled in the column.
    let Some(first_root) = paths.first() else {
        return Ok(None);
    };
    let json_escaped = first_root.to_string_lossy().replace('\\', "\\\\");
    let mut pattern = String::new();
    for character in json_escaped.chars() {
        if matches!(character, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    let candidates = state.with_database(history_path, |connection| {
        select_entries(
            connection,
            "WHERE kind = 'files' AND sources LIKE ? ESCAPE '\\'",
            &crate::store::newest_first_sql(None, 0),
            &[rusqlite::types::Value::Text(format!("%{pattern}%"))],
        )
    })?;
    for candidate in candidates {
        if file_entry_signature(&candidate) == signature {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

/// The frontend renders lists of hundreds of entries; shipping their trees
/// would mean tens of thousands of nodes per refresh.
pub(crate) fn lightweight_entry(entry: &ClipboardEntry) -> ClipboardEntry {
    // html/rtf can be hundreds of kilobytes per rich-text entry and the list
    // never renders them, so they stay behind `get_entry`. Built field by
    // field: a struct-update clone would copy those strings just to drop them.
    let mut lightweight = ClipboardEntry {
        id: entry.id.clone(),
        kind: entry.kind.clone(),
        content: entry.content.clone(),
        html: None,
        rtf: None,
        file_info: None,
        image_info: None,
        source_device_id: entry.source_device_id.clone(),
        created_at: entry.created_at.clone(),
        summary: entry.summary.clone(),
        sources: LocalSources::default(),
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
    let RichText { text, html, rtf } = rich_text;
    let state = app.state::<AppState>();
    let entry = {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        if history.last_clipboard == signature {
            return Ok(());
        }
        // The signatures are recorded before the transaction so the metadata
        // write inside it persists them; on failure they roll back, keeping
        // memory aligned with the database the transaction never touched.
        let previous_signatures = (
            history.last_clipboard.clone(),
            history.last_file_signature.clone(),
            history.last_image_signature.clone(),
        );
        history.last_clipboard = signature;
        history.last_file_signature.clear();
        history.last_image_signature.clear();
        let device_id = history.device_id.clone();
        let created_at = Utc::now().to_rfc3339();
        let extra = ClipboardEntryExtra {
            html,
            rtf,
            file_info: None,
            image_info: None,
        };
        let payload = extra.json()?;
        let path = history_path_for_key(&state.histories_dir, &history.active_history);
        // One transaction: queue row, dedup, entry row and metadata land
        // together, so a crash cannot leave a row without its queue entry. The
        // seq the queue returns is the entry's local id until the server's is
        // adopted; without a queue row there is nothing to sync later, so a
        // failure here skips the capture entirely.
        let entry = match state.with_database(&path, |connection| {
            let transaction = connection.transaction().map_err(|error| error.to_string())?;
            let seq = enqueue(&transaction, "text", &text, &payload, &created_at)?;
            let mut entry = new_entry(seq, "text", text.clone(), device_id.clone());
            entry.html = extra.html.clone();
            entry.rtf = extra.rtf.clone();
            entry.created_at = created_at.clone();
            // Text dedup ignores kind — same as the in-memory retain below.
            // The duplicate rows go first, queue rows included, so the fresh
            // insert cannot collide with itself.
            let duplicates = select_entries(&transaction, "WHERE content = ?", "", &[
                rusqlite::types::Value::Text(entry.content.clone()),
            ])?
            .into_iter()
            .map(|item| item.id)
            .collect::<Vec<_>>();
            delete_rows_for(&transaction, &duplicates)?;
            delete_entries_by_ids(&transaction, &duplicates)?;
            upsert_entry_row(&transaction, &entry)?;
            save_metadata(&transaction, &history)?;
            transaction.commit().map_err(|error| error.to_string())?;
            Ok(entry)
        }) {
            Ok(entry) => entry,
            Err(error) => {
                history.last_clipboard = previous_signatures.0;
                history.last_file_signature = previous_signatures.1;
                history.last_image_signature = previous_signatures.2;
                eprintln!("ClipRoam: 记录剪贴板条目失败：{error}");
                return Ok(());
            }
        };
        entry
    };
    // Text has no contents to hash, so it is publishable the moment it lands —
    // the frontend drains the queue whenever an entry is created.
    app.emit("cliproam://entry-created", lightweight_entry(&entry))
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
    let entry = {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        if history.last_file_signature == signature {
            return Ok(());
        }
        // Signatures roll back on a failed write, keeping memory aligned with
        // the database the transaction never touched.
        let previous_signatures = (
            history.last_clipboard.clone(),
            history.last_file_signature.clone(),
            history.last_image_signature.clone(),
        );
        history.last_file_signature = signature.clone();
        history.last_clipboard.clear();
        history.last_image_signature.clear();
        let cache_dir = crate::active_cache_dir(&state, &history);
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        let device_id = history.device_id.clone();
        let created_at = Utc::now().to_rfc3339();
        let content = describe_roots(&collected.file_info);
        let reusable = find_reusable_files_entry(&state, &history_path, &paths, &signature)?;
        let mut entry = match reusable {
            Some(mut existing) => {
                existing.created_at = created_at;
                // One transaction: the entry row (new timestamp, so it moves
                // back to the top) and the metadata; a reused entry keeps its
                // id — and its queue row when it is still unpublished.
                if let Err(error) = state.with_database(&history_path, |connection| {
                    let transaction = connection.transaction().map_err(|error| error.to_string())?;
                    upsert_entry_row(&transaction, &existing)?;
                    save_metadata(&transaction, &history)?;
                    transaction.commit().map_err(|error| error.to_string())?;
                    Ok(())
                }) {
                    history.last_clipboard = previous_signatures.0;
                    history.last_file_signature = previous_signatures.1;
                    history.last_image_signature = previous_signatures.2;
                    eprintln!("ClipRoam: 记录剪贴板条目失败：{error}");
                    return Ok(());
                }
                existing
            }
            None => {
                // The tree goes into the queue with unresolved content ids
                // (`f: ""`); the sync drain resolves them before publishing.
                let extra = ClipboardEntryExtra {
                    html: None,
                    rtf: None,
                    file_info: Some(collected.file_info.clone()),
                    image_info: None,
                };
                let payload = extra.json()?;
                let mut entry = new_entry(0, "files", content, device_id);
                entry.created_at = created_at;
                entry.file_info = Some(collected.file_info);
                entry.sources = collected.sources;
                if let Err(error) = state.with_database(&history_path, |connection| {
                    let transaction = connection.transaction().map_err(|error| error.to_string())?;
                    let seq = enqueue(
                        &transaction,
                        "files",
                        &entry.content,
                        &payload,
                        &entry.created_at,
                    )?;
                    entry.id = crate::pending::temp_entry_id(seq);
                    upsert_entry_row(&transaction, &entry)?;
                    save_metadata(&transaction, &history)?;
                    transaction.commit().map_err(|error| error.to_string())?;
                    Ok(())
                }) {
                    history.last_clipboard = previous_signatures.0;
                    history.last_file_signature = previous_signatures.1;
                    history.last_image_signature = previous_signatures.2;
                    eprintln!("ClipRoam: 记录剪贴板条目失败：{error}");
                    return Ok(());
                }
                entry
            }
        };
        refresh_summary(&mut entry, &history.cached_files, &history.uploaded_files, &cache_dir);
        lightweight_entry(&entry)
    };
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
        // Signatures roll back on a failed write, keeping memory aligned with
        // the database the transaction never touched.
        let previous_signatures = (
            history.last_clipboard.clone(),
            history.last_file_signature.clone(),
            history.last_image_signature.clone(),
        );
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
        // entry is publishable as soon as it lands. One transaction covers the
        // queue row, the entry row and the metadata.
        let extra = ClipboardEntryExtra {
            html: None,
            rtf: None,
            file_info: None,
            image_info: Some(image_info.clone()),
        };
        let payload = extra.json()?;
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        let entry = match state.with_database(&history_path, |connection| {
            let transaction = connection.transaction().map_err(|error| error.to_string())?;
            let seq = enqueue(&transaction, "image", &content, &payload, &created_at)?;
            let mut entry = new_entry(seq, "image", content, device_id);
            entry.created_at = created_at;
            entry.image_info = Some(image_info);
            upsert_entry_row(&transaction, &entry)?;
            save_metadata(&transaction, &history)?;
            transaction.commit().map_err(|error| error.to_string())?;
            Ok(entry)
        }) {
            Ok(entry) => entry,
            Err(error) => {
                history.last_clipboard = previous_signatures.0;
                history.last_file_signature = previous_signatures.1;
                history.last_image_signature = previous_signatures.2;
                eprintln!("ClipRoam: 记录剪贴板条目失败：{error}");
                return Ok(());
            }
        };
        let mut entry = entry;
        refresh_summary(&mut entry, &history.cached_files, &history.uploaded_files, &cache_dir);
        lightweight_entry(&entry)
    };
    app.emit("cliproam://entry-created", entry)
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
