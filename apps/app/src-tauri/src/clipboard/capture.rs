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
    collect_tree, describe_roots, file_entry_signature, file_signature,
    ClipboardEntry, ClipboardEntryExtra, ImageInfo, LocalSources,
};
use crate::utils::{fnv1a, hash_bytes};
use crate::file::upload_image_path;
use crate::pending::enqueue_pending_entry;
use crate::store::{
    history_path_for_key, save_metadata, select_entries, upsert_entry_row, HistoryData,
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
    let pattern = crate::utils::escape_like(&json_escaped);
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

/// 本次捕获命中哪一类去重签名。三类互斥：记录一类时清空另两类，所以写
/// 入剪贴板（激活）不会让监控把它当成新条目再捕获一遍。
enum CapturedSignature {
    Clipboard(String),
    File(String),
    Image(String),
}

impl CapturedSignature {
    fn matches(&self, history: &HistoryData) -> bool {
        match self {
            Self::Clipboard(signature) => history.last_clipboard == *signature,
            Self::File(signature) => history.last_file_signature == *signature,
            Self::Image(signature) => history.last_image_signature == *signature,
        }
    }

    fn record(&self, history: &mut HistoryData) {
        let (clipboard, file, image) = match self {
            Self::Clipboard(signature) => (signature.clone(), String::new(), String::new()),
            Self::File(signature) => (String::new(), signature.clone(), String::new()),
            Self::Image(signature) => (String::new(), String::new(), signature.clone()),
        };
        history.last_clipboard = clipboard;
        history.last_file_signature = file;
        history.last_image_signature = image;
    }

    fn restore(&self, previous: (String, String, String), history: &mut HistoryData) {
        history.last_clipboard = previous.0;
        history.last_file_signature = previous.1;
        history.last_image_signature = previous.2;
    }
}

/// 三个捕获路径共用的去重与事务骨架：命中签名直接跳过；否则快照三元组、
/// 记录新签名（让事务内的 metadata 写把它们落库）、执行 `write`；失败时
/// 回滚签名，让内存与事务从未触及的数据库保持一致，并把错误记进日志后
/// 吞掉——监控线程没有别的上报途径，且没有队列行就无事可同步。
fn capture_transaction(
    history: &mut HistoryData,
    signature: CapturedSignature,
    write: impl FnOnce(&mut HistoryData) -> Result<(), String>,
) -> Result<(), String> {
    if signature.matches(history) {
        return Ok(());
    }
    let previous = (
        history.last_clipboard.clone(),
        history.last_file_signature.clone(),
        history.last_image_signature.clone(),
    );
    signature.record(history);
    if let Err(error) = write(history) {
        signature.restore(previous, history);
        eprintln!("ClipRoam: 记录剪贴板条目失败：{error}");
    }
    Ok(())
}

pub(crate) fn capture_text(app: &AppHandle, rich_text: RichText) -> Result<(), String> {
    if rich_text.text.trim().is_empty() {
        return Ok(());
    }
    let signature = rich_text_signature(&rich_text);
    let RichText { text, html, rtf } = rich_text;
    let state = app.state::<AppState>();
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let created_at = Utc::now().to_rfc3339();
        let extra = ClipboardEntryExtra {
            html,
            rtf,
            file_info: None,
            image_info: None,
            local_sources: LocalSources::default(),
        };
        let payload = extra.json()?;
        capture_transaction(&mut history, CapturedSignature::Clipboard(signature), |history| {
            let path = history_path_for_key(&state.histories_dir, &history.active_history);
            // One transaction: queue row, dedup and metadata land together, so
            // a crash cannot leave a half-written capture. A failure here skips
            // the capture entirely — without a queue row there is nothing to sync.
            state.with_database(&path, |connection| {
                let transaction = connection.transaction().map_err(|error| error.to_string())?;
                // Queue dedup ignores kind: the same content waiting to be synced
                // collapses into this new row. Published duplicates stay alone —
                // the server dedups by content, so the publish refreshes that
                // row's timestamp and the echo moves it back to the top.
                transaction
                    .execute("DELETE FROM pending_entries WHERE content = ?", [&text])
                    .map_err(|error| error.to_string())?;
                enqueue_pending_entry(&transaction, "text", &text, &payload, &created_at)?;
                save_metadata(&transaction, &history)?;
                transaction.commit().map_err(|error| error.to_string())?;
                Ok(())
            })
        })?;
    }
    // Text has no contents to hash, so it is publishable the moment it lands —
    // the frontend drains the queue whenever an entry is created.
    app.emit("cliproam://entry-created", ())
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
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let created_at = Utc::now().to_rfc3339();
        capture_transaction(&mut history, CapturedSignature::File(signature.clone()), |history| {
            let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
            // A re-copy of the same roots refreshes the published entry's
            // timestamp instead of queueing the tree a second time.
            let reusable = find_reusable_files_entry(&state, &history_path, &paths, &signature)?;
            match reusable {
                Some(mut existing) => {
                    existing.created_at = created_at;
                    let outcome = state.with_database(&history_path, |connection| {
                        let transaction = connection.transaction().map_err(|error| error.to_string())?;
                        upsert_entry_row(&transaction, &existing)?;
                        save_metadata(&transaction, &history)?;
                        transaction.commit().map_err(|error| error.to_string())?;
                        Ok(())
                    });
                    // A row write lands, so the derived file-id cache is stale
                    // (conservatively — only `created_at` changed here).
                    history.file_ids = None;
                    outcome
                }
                None => {
                    // The tree goes into the queue with unresolved content ids
                    // (`f: ""`) and the raw source paths; the sync drain resolves
                    // both before publishing. One transaction: queue row and
                    // metadata land together.
                    let content = describe_roots(&collected.file_info);
                    let extra = ClipboardEntryExtra {
                        html: None,
                        rtf: None,
                        file_info: Some(collected.file_info),
                        image_info: None,
                        local_sources: collected.sources,
                    };
                    let payload = extra.json()?;
                    state.with_database(&history_path, |connection| {
                        let transaction = connection.transaction().map_err(|error| error.to_string())?;
                        enqueue_pending_entry(&transaction, "files", &content, &payload, &created_at)?;
                        save_metadata(&transaction, &history)?;
                        transaction.commit().map_err(|error| error.to_string())?;
                        Ok(())
                    })
                }
            }
        })?;
    }
    // The queue row lands either resolved (reuse) or placeholder (new); the
    // frontend refreshes the pending view and starts the drain from the event.
    app.emit("cliproam://entry-created", ())
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
    let created_at = Utc::now().to_rfc3339();
    let content = format!("截图（{width} × {height}）");
    // Bytes are hashed already, so the queued payload is complete and the
    // entry is publishable the moment it lands.
    let extra = ClipboardEntryExtra {
        html: None,
        rtf: None,
        file_info: None,
        image_info: Some(ImageInfo {
            file_id: file_id.clone(),
            size: webp.len() as u64,
            thumbnail: thumbnail.unwrap_or_default(),
        }),
        local_sources: LocalSources::default(),
    };
    let payload = extra.json()?;
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        capture_transaction(&mut history, CapturedSignature::Image(signature), |history| {
            let cache_dir = crate::active_cache_dir(&state, &history);
            let image_path = upload_image_path(&cache_dir, &file_id).ok_or_else(|| "内容标识不合法".to_string())?;
            if let Some(parent) = image_path.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            if !image_path.is_file() {
                fs::write(&image_path, &webp).map_err(|error| error.to_string())?;
            }
            history.cached_files.insert(file_id.clone());
            let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
            // One transaction covers the queue row and the metadata.
            state.with_database(&history_path, |connection| {
                let transaction = connection.transaction().map_err(|error| error.to_string())?;
                enqueue_pending_entry(&transaction, "image", &content, &payload, &created_at)?;
                save_metadata(&transaction, &history)?;
                transaction.commit().map_err(|error| error.to_string())?;
                Ok(())
            })
        })?;
    }
    app.emit("cliproam://entry-created", ())
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
