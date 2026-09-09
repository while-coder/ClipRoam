//! Applying remote (server-originated) file-availability changes to the local
//! history. Entry writes and deletions live in the `entry` module.

use std::collections::HashSet;
use tauri::{AppHandle, Emitter, State};

use crate::store::{
    history_path_for_key, mark_files_uploaded as store_mark_files_uploaded,
};
use crate::AppState;

/// File ids this device knows nothing about: neither a local blob in the cache
/// nor an "available" mark from the server pool. The sync flow queries server
/// storage status only for these, so locally known contents never ride a
/// `/files/query` request.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn filter_unknown_file_ids(
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
pub(crate) fn mark_files_uploaded(
    app: AppHandle,
    state: State<'_, AppState>,
    file_ids: Vec<String>,
) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        let uploaded = file_ids.into_iter().collect::<Vec<_>>();
        history.uploaded_files.extend(uploaded.iter().cloned());
        state.with_database(&history_path, |connection| {
            store_mark_files_uploaded(connection, &uploaded);
            Ok(())
        })?;
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

/// Server storage is content-addressed, so another device can finish uploading
/// a file after this entry was already received locally. Availability is read
/// back into every summary on the next read, so only the mark needs writing.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn mark_file_available(
    app: AppHandle,
    state: State<'_, AppState>,
    file_id: String,
) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        if history.uploaded_files.contains(&file_id) {
            return Ok(());
        }
        history.uploaded_files.insert(file_id.clone());
        state.with_database(&history_path, |connection| {
            store_mark_files_uploaded(connection, std::slice::from_ref(&file_id));
            Ok(())
        })?;
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}
