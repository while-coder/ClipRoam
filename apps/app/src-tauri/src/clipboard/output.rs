//! Writing entries back to the OS clipboard, including the paste strategy
//! that decides when remote contents must be materialized first.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, State};

use crate::content::{file_signature, readable_path, rebuild_tree, ClipboardEntry, MissingFile};
use crate::store::{cached_source_for, open_history_database, refresh_entry_summary, history_path_for_key};
use crate::history::entry_contents_of;
use crate::{active_cache_dir, save_active_history, AppState};

use super::capture::{image_signature, rich_text_signature, safe_file_name, RichText};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilePasteStrategy {
    VirtualStream,
    MaterializedPaths,
}

impl FilePasteStrategy {
    pub(crate) fn for_entry(entry: &ClipboardEntry) -> Self {
        if entry.kind == "files" && crate::platforms::supports_virtual_file_paste(entry) {
            return Self::VirtualStream;
        }
        Self::MaterializedPaths
    }

    pub(crate) fn requires_complete_content(self, kind: &str) -> bool {
        kind == "image" || (kind == "files" && self == Self::MaterializedPaths)
    }
}


pub(crate) enum ClipboardPayload {
    Text(RichText),
    Files(Vec<String>),
    /// 仅在 `FilePasteStrategy::VirtualStream`（当前只有 Windows）下构造；
    /// 非 Windows 上写入时由平台适配层返回错误。
    VirtualFiles(Box<ClipboardEntry>),
    Image(Vec<u8>),
}

/// A snapshot taken under the history lock so file dialogs and disk work never
/// block the clipboard monitor.
pub(crate) struct EntrySnapshot {
    pub entry: ClipboardEntry,
    cached: HashSet<String>,
    /// Contents neither a cache blob nor a surviving local source covers,
    /// resolved against the hash cache once at snapshot time: a file this
    /// machine hashed before can stand in for the content and spare a
    /// download. Target names come from the entry's tree, never from these
    /// source files, so a differently named stand-in pastes under the
    /// original name.
    hash_sources: HashMap<String, PathBuf>,
    pub cache_dir: PathBuf,
}

pub(crate) fn snapshot_entry(state: &AppState, entry_id: &str) -> Result<EntrySnapshot, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let cache_dir = active_cache_dir(state, &history);
    let entry = history
        .find(entry_id)
        .cloned()
        .ok_or_else(|| "剪贴板记录不存在".to_string())?;
    let hash_sources = match open_history_database(&history_path_for_key(
        &state.histories_dir,
        &history.active_history,
    )) {
        Ok(connection) => entry_contents_of(&entry)
            .into_iter()
            .filter(|(file_id, _)| readable_path(&cache_dir, &history.cached_files, &entry, file_id).is_none())
            .filter_map(|(file_id, _)| {
                cached_source_for(&connection, &file_id).map(|path| (file_id, path))
            })
            .collect(),
        Err(_) => HashMap::new(),
    };
    Ok(EntrySnapshot {
        entry,
        cached: history.cached_files.clone(),
        hash_sources,
        cache_dir,
    })
}

impl EntrySnapshot {
    pub(crate) fn resolve(&self, file_id: &str) -> Option<PathBuf> {
        readable_path(&self.cache_dir, &self.cached, &self.entry, file_id)
            .or_else(|| self.hash_sources.get(file_id).cloned())
    }
}

pub(crate) fn missing_files(snapshot: &EntrySnapshot) -> Vec<MissingFile> {
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

pub(crate) fn refresh_snapshot_summary(state: &AppState, snapshot: &EntrySnapshot, entry_id: &str) -> Result<(), String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    refresh_entry_summary(&mut history, entry_id, &snapshot.cache_dir);
    Ok(())
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

/// Records which signature suppresses re-capturing what was just written,
/// clearing the other two so a different payload kind is still captured.
fn record_activation_signature(history: &mut crate::store::HistoryData, payload: &ClipboardPayload) {
    let (file, clipboard, image) = match payload {
        ClipboardPayload::Files(paths) => {
            let paths = paths.iter().map(PathBuf::from).collect::<Vec<_>>();
            (file_signature(&paths), String::new(), String::new())
        }
        ClipboardPayload::VirtualFiles(_) => (String::new(), String::new(), String::new()),
        ClipboardPayload::Image(image) => (String::new(), String::new(), image_signature(image)),
        ClipboardPayload::Text(rich_text) => (String::new(), rich_text_signature(rich_text), String::new()),
    };
    history.last_file_signature = file;
    history.last_clipboard = clipboard;
    history.last_image_signature = image;
}

/// Writes a live clipboard activation received from another device without
/// synthesizing Paste. File-list entries are deliberately excluded: they stay
/// in history until the user explicitly chooses where to paste or save them.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn activate_remote_entry(
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<(), String> {
    let snapshot = snapshot_entry(&state, &entry_id)?;
    let payload = match snapshot.entry.kind.as_str() {
        "files" => return Err("文件和文件夹不会自动写入漫游剪贴板".to_string()),
        "image" => image_payload(&snapshot)?,
        _ => text_payload(&snapshot.entry),
    };

    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        record_activation_signature(&mut history, &payload);
        save_active_history(&state, &history)?;
    }

    match payload {
        ClipboardPayload::Text(rich_text) => crate::platforms::write_clipboard_text(&app, &rich_text),
        ClipboardPayload::Image(image) => crate::platforms::write_clipboard_image(&app, &image),
        _ => unreachable!("file activations are rejected above"),
    }
}

pub(crate) fn apply_clipboard_entry(
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
                        ClipboardPayload::VirtualFiles(Box::new(snapshot.entry.clone()))
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
        "image" => image_payload(&snapshot)?,
        _ => text_payload(&snapshot.entry),
    };

    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        record_activation_signature(&mut history, &payload);
        save_active_history(&state, &history)?;
    }

    match payload {
        ClipboardPayload::Text(rich_text) => crate::platforms::write_clipboard_text(&app, &rich_text)?,
        ClipboardPayload::Files(paths) => crate::platforms::write_clipboard_files(&app, &paths)?,
        ClipboardPayload::VirtualFiles(entry) => {
            crate::platforms::set_virtual_file_clipboard(&app, window.label(), *entry)?
        }
        ClipboardPayload::Image(image) => crate::platforms::write_clipboard_image(&app, &image)?,
    }

    crate::platforms::deliver_paste(&window, synthesize)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn copy_entry(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<(), String> {
    apply_clipboard_entry(window, app, state, entry_id, false)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn paste_entry(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<(), String> {
    if crate::platforms::requires_paste_window() && window.label() != "paste" {
        return Err("只有快捷粘贴窗口可以执行自动粘贴".to_string());
    }

    apply_clipboard_entry(window, app, state, entry_id, true)
}
