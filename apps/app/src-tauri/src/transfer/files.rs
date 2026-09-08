//! Serving entry contents to the frontend: chunked reads for upload and the
//! availability snapshots that drive downloads.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Serialize;
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
};
use tauri::State;

use crate::clipboard::output::{
    missing_files, refresh_snapshot_summary, snapshot_entry, FilePasteStrategy,
};
use crate::content::{local_source_was_lost, readable_path};
use crate::history::entry_contents_of;
use crate::{active_cache_dir, AppState};

/// Largest chunk served per `read_file_chunk` call.
const FILE_CHUNK_LIMIT: usize = 128 * 1024;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EntryFileCandidate {
    file_id: String,
    size: u64,
    uploaded: bool,
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn list_entry_files(
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

/// Contents this machine cannot read yet, de-duplicated — the frontend turns
/// each one into a download.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn prepare_entry_files(state: State<'_, AppState>, entry_id: String) -> Result<Vec<crate::transfer::save::MissingFile>, String> {
    let snapshot = snapshot_entry(&state, &entry_id)?;
    refresh_snapshot_summary(&state, &snapshot, &entry_id)?;
    Ok(missing_files(&snapshot))
}

/// Returns only the contents that must exist before this platform can start a
/// paste. The frontend does not need to know which operating system it runs on.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn prepare_paste_entry(state: State<'_, AppState>, entry_id: String) -> Result<Vec<crate::transfer::save::MissingFile>, String> {
    let snapshot = snapshot_entry(&state, &entry_id)?;
    refresh_snapshot_summary(&state, &snapshot, &entry_id)?;
    if FilePasteStrategy::for_entry(&snapshot.entry).requires_complete_content(&snapshot.entry.kind) {
        Ok(missing_files(&snapshot))
    } else {
        Ok(Vec::new())
    }
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn read_file_chunk(
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
