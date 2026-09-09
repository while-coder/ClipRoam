//! 条目更新与删除：远端来源的写入（服务器回显 upsert）与服务器确认的删除。

use tauri::{AppHandle, Emitter, State};

use crate::content::ClipboardEntry;
use crate::file::collect_local_garbage;
use crate::store::{delete_entries_by_ids, history_path_for_key, upsert_entry_row};
use crate::AppState;

/// Reconciling a fresh install can deliver hundreds of remote entries at once;
/// a single lock, save and event keeps that from locking up the windows.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn upsert_remote_entries(
    app: AppHandle,
    state: State<'_, AppState>,
    entries: Vec<ClipboardEntry>,
) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        // The rows go in ascending created_at order, so within one millisecond
        // the newest insert gets the highest rowid and the created_ms DESC,
        // rowid DESC index yields a stable newest-first order.
        let mut upserts = entries;
        upserts.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        state.with_database(&history_path, |connection| {
            let transaction = connection.transaction().map_err(|error| error.to_string())?;
            for entry in &upserts {
                upsert_entry_row(&transaction, entry)?;
            }
            transaction.commit().map_err(|error| error.to_string())?;
            Ok(())
        })?;
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

/// Applies a server-confirmed deletion: drops the entry, then frees the blobs
/// it referenced.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn remove_remote_entry(app: AppHandle, state: State<'_, AppState>, entry_id: String) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let path = history_path_for_key(&state.histories_dir, &history.active_history);
        state.with_database(&path, |connection| {
            let transaction = connection.transaction().map_err(|error| error.to_string())?;
            delete_entries_by_ids(&transaction, std::slice::from_ref(&entry_id))?;
            transaction.commit().map_err(|error| error.to_string())?;
            Ok(())
        })?;
        // Dropping references is what frees disk space, so the sweep runs here.
        let _ = state.with_database(&path, |connection| {
            collect_local_garbage(connection, &state.histories_dir, &mut history)
        });
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}
