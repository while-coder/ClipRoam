//! History queries and lifecycle commands: listing, reading, refreshing,
//! deleting.

use std::collections::HashSet;
use tauri::{AppHandle, Emitter, State};

use crate::clipboard::capture::lightweight_entry;
use crate::content::{tree_contents, ClipboardEntry};
use crate::store::{collect_local_garbage, refresh_entry_summary, temp_entry_seq, HistoryData};
use crate::{active_cache_dir, save_active_history, AppState};

/// Every content an entry references, whichever kind carries it.
pub(crate) fn entry_contents_of(entry: &ClipboardEntry) -> Vec<(String, u64)> {
    match (&entry.file_info, &entry.image_info) {
        (Some(file_info), _) => tree_contents(file_info),
        (None, Some(image)) => vec![(image.file_id.clone(), image.size)],
        (None, None) => Vec::new(),
    }
}

pub(crate) fn entry_references(entry: &ClipboardEntry, file_id: &str) -> bool {
    entry_contents_of(entry)
        .into_iter()
        .any(|(id, _)| id == file_id)
}

#[tauri::command]
pub(crate) fn list_entries(state: State<'_, AppState>) -> Result<Vec<ClipboardEntry>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    Ok(history
        .active_entries()
        .iter()
        .map(lightweight_entry)
        .collect())
}

/// The full entry, tree included — used when publishing to the server.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn get_entry(state: State<'_, AppState>, entry_id: String) -> Result<ClipboardEntry, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    history
        .find(&entry_id)
        .cloned()
        .ok_or_else(|| "剪贴板记录不存在".to_string())
}

#[tauri::command]
pub(crate) fn get_device(state: State<'_, AppState>) -> Result<(String, String), String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    Ok((history.device_id.clone(), history.device_name.clone()))
}

/// Recomputes one entry's aggregates. Downloads deliberately skip this so that
/// finishing a file stays O(1); the caller refreshes once the batch is done.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn refresh_entry(state: State<'_, AppState>, entry_id: String) -> Result<(), String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    let cache_dir = active_cache_dir(&state, &history);
    refresh_entry_summary(&mut history, &entry_id, &cache_dir);
    Ok(())
}

/// Removes the given ids from the active history, tombstones every published
/// id and frees the blobs they referenced.
fn remove_entries_and_enqueue_deletions(
    state: &AppState,
    history: &mut HistoryData,
    entry_ids: &[String],
) -> Result<(), String> {
    let removed = entry_ids.iter().cloned().collect::<HashSet<_>>();
    let existing = history
        .active_entries()
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<HashSet<_>>();
    history
        .active_entries_mut()
        .retain(|entry| !removed.contains(&entry.id));
    for entry_id in entry_ids {
        // The server only knows published (numeric) ids; a temporary id was
        // never uploaded, so removing it merely drops its queue row on the
        // next save.
        if existing.contains(entry_id) && temp_entry_seq(entry_id).is_none() {
            history.pending_deletions.insert(entry_id.clone());
        }
    }
    save_active_history(state, history)?;
    // Dropping references is what frees disk space, so the sweep runs here.
    let _ = collect_local_garbage(&state.histories_dir, history);
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn delete_entry(app: AppHandle, state: State<'_, AppState>, entry_id: String) -> Result<(), String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    remove_entries_and_enqueue_deletions(&state, &mut history, &[entry_id])?;
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn clear_history(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    let entry_ids = history
        .active_entries()
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<Vec<_>>();
    remove_entries_and_enqueue_deletions(&state, &mut history, &entry_ids)?;
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}
