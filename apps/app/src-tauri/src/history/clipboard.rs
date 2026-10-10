//! Writing entries back to the OS clipboard, including the paste strategy
//! that decides when remote contents must be materialized first.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, State};

use crate::content::{file_signature, readable_path, rebuild_tree, ClipboardEntry};
use crate::file::cached_source_for;
use crate::store::{save_metadata, select_entry};
use crate::content::entry_contents_of;
use crate::utils::sanitize_name_component;
use crate::AppState;

use crate::content::clipboard::{image_signature, rich_text_signature, RichText};

pub(crate) enum ClipboardPayload {
    Text(RichText),
    Files(Vec<String>),
    Image(Vec<u8>),
}

/// A snapshot taken under the history lock so file dialogs and disk work never
/// block the clipboard monitor.
pub(crate) struct EntrySnapshot {
    pub entry: ClipboardEntry,
    /// Contents neither a cache blob nor a surviving local source covers,
    /// resolved against the hash cache once at snapshot time: a file this
    /// machine hashed before can stand in for the content and spare a
    /// download. Target names come from the entry's tree, never from these
    /// source files, so a differently named stand-in pastes under the
    /// original name.
    hash_sources: HashMap<String, PathBuf>,
    pub cache_dir: PathBuf,
}

pub(crate) fn snapshot_entry_for(entry_id: &str, account: &crate::account::AccountContext) -> Result<EntrySnapshot, String> {
    let cache_dir = account.cache_dir.clone();
    let entry = account.with_database(|connection| select_entry(connection, entry_id))?
        .ok_or_else(|| "剪贴板记录不存在".to_string())?;
    let hash_sources = account.with_database(|connection| {
            Ok(entry_contents_of(&entry)
                .into_iter()
                .filter(|(file_id, _)| readable_path(&cache_dir, &entry, file_id).is_none())
                .filter_map(|(file_id, _)| {
                    cached_source_for(connection, &file_id).map(|path| (file_id, path))
                })
                .collect::<HashMap<String, PathBuf>>())
        })
        .unwrap_or_default();
    Ok(EntrySnapshot {
        entry,
        hash_sources,
        cache_dir,
    })
}

impl EntrySnapshot {
    pub(crate) fn resolve(&self, file_id: &str) -> Option<PathBuf> {
        readable_path(&self.cache_dir, &self.entry, file_id)
            .or_else(|| self.hash_sources.get(file_id).cloned())
    }
}

fn image_payload(snapshot: &EntrySnapshot) -> Result<ClipboardPayload, String> {
    let file_id = snapshot
        .entry
        .image_info
        .as_ref()
        .map(|image| image.file_id.clone())
        .ok_or_else(|| "图片内容不可用".to_string())?;
    let path = snapshot
        .resolve(&file_id)
        .ok_or_else(|| "图片内容不可用".to_string())?;
    Ok(ClipboardPayload::Image(fs::read(path).map_err(|error| error.to_string())?))
}

fn text_payload(entry: &ClipboardEntry) -> ClipboardPayload {
    ClipboardPayload::Text(RichText {
        text: entry.content.clone(),
        html: entry.html.clone(),
        rtf: entry.rtf.clone(),
    })
}

/// The signature triple that suppresses re-capturing what is about to be
/// written, with the other two cleared so a different payload kind is still
/// captured.
fn activation_signature(payload: &ClipboardPayload) -> (String, String, String) {
    match payload {
        ClipboardPayload::Files(paths) => {
            let paths = paths.iter().map(PathBuf::from).collect::<Vec<_>>();
            (file_signature(&paths), String::new(), String::new())
        }
        ClipboardPayload::Image(image) => (String::new(), String::new(), image_signature(image)),
        ClipboardPayload::Text(rich_text) => (String::new(), rich_text_signature(rich_text), String::new()),
    }
}

/// Records which signature suppresses re-capturing what was just written,
/// clearing the other two so a different payload kind is still captured.
fn record_activation_signature(history: &mut crate::store::HistoryData, signature: (String, String, String)) {
    let (file, clipboard, image) = signature;
    history.last_file_signature = file;
    history.last_clipboard = clipboard;
    history.last_image_signature = image;
}

/// Writes the payload and records the activation signature as one serialized
/// step: the signature is recorded first and the clipboard write happens
/// inside the history lock, so the monitor — which locks the same mutex
/// between reading the clipboard and capturing — can never observe the new
/// clipboard value against the old signatures and re-capture what we just
/// wrote (the old write-then-record order left exactly that window, and it
/// stretched whenever the metadata write below had to wait on the database).
/// On write failure the in-memory signature rolls back while the database is
/// untouched, so the old clipboard content still matches its old signature.
fn activate_with_signature(
    state: &AppState,
    signature: (String, String, String),
    account: &crate::account::AccountContext,
    write: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    if history.active_history.as_deref() != Some(account.key.as_str()) {
        return Err("账号会话已结束，取消写入系统剪贴板".to_string());
    }
    let previous = (
        history.last_clipboard.clone(),
        history.last_file_signature.clone(),
        history.last_image_signature.clone(),
    );
    record_activation_signature(&mut history, signature);
    if let Err(error) = write() {
        history.last_clipboard = previous.0;
        history.last_file_signature = previous.1;
        history.last_image_signature = previous.2;
        return Err(error);
    }
    // Only the activation signatures changed — persist the metadata rows.
    account.with_database(|connection| save_metadata(connection, &history))
}

/// Writes a live clipboard activation received from another device without
/// synthesizing Paste. File-list entries are deliberately excluded: they stay
/// in history until the user explicitly chooses where to paste or save them.
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn activate_remote_entry(
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
    session_id: String,
) -> Result<(), String> {
    let account = state.account(&session_id)?;
    let snapshot = snapshot_entry_for(&entry_id, &account)?;
    let payload = match snapshot.entry.kind.as_str() {
        "files" => return Err("文件和文件夹不会自动写入漫游剪贴板".to_string()),
        "image" => image_payload(&snapshot)?,
        _ => text_payload(&snapshot.entry),
    };
    let signature = activation_signature(&payload);

    activate_with_signature(&state, signature, &account, || match payload {
        ClipboardPayload::Text(rich_text) => crate::platforms::write_clipboard_text(&app, &rich_text),
        ClipboardPayload::Image(image) => crate::platforms::write_clipboard_image(&app, &image),
        _ => unreachable!("file activations are rejected above"),
    })?;
    Ok(())
}

pub(crate) fn apply_clipboard_entry(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
    synthesize: bool,
    session_id: String,
) -> Result<(), String> {
    let account = state.account(&session_id)?;
    let snapshot = snapshot_entry_for(&entry_id, &account)?;
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
                let view = snapshot
                    .cache_dir
                    .join("views")
                    .join(sanitize_name_component(&snapshot.entry.id));
                let _ = fs::remove_dir_all(&view);
                // Real copies, not hard links: the view is handed to
                // other applications, and an in-place edit there must
                // not reach back into the content-addressed cache blob
                // (the filename is the sha256 — corrupted bytes would
                // then be trusted for every entry sharing the content).
                rebuild_tree(&view, file_info, &|file_id| snapshot.resolve(file_id), false)?;
                let paths = file_info
                    .keys()
                    .map(|root| view.join(root).display().to_string())
                    .collect();
                ClipboardPayload::Files(paths)
            }
        }
        "image" => image_payload(&snapshot)?,
        _ => text_payload(&snapshot.entry),
    };

    let signature = activation_signature(&payload);

    activate_with_signature(&state, signature, &account, || match payload {
        ClipboardPayload::Text(rich_text) => crate::platforms::write_clipboard_text(&app, &rich_text),
        ClipboardPayload::Files(paths) => crate::platforms::write_clipboard_files(&app, &paths),
        ClipboardPayload::Image(image) => crate::platforms::write_clipboard_image(&app, &image),
    })?;

    crate::platforms::deliver_paste(&window, synthesize)
}

#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn copy_entry(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
    session_id: String,
) -> Result<(), String> {
    apply_clipboard_entry(window, app, state, entry_id, false, session_id)
}

#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn paste_entry(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
    session_id: String,
) -> Result<(), String> {
    if crate::platforms::requires_paste_window() && window.label() != "paste" {
        return Err("只有快捷粘贴窗口可以执行自动粘贴".to_string());
    }

    apply_clipboard_entry(window, app, state, entry_id, true, session_id)
}
