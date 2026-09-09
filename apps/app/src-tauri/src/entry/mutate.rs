//! 条目更新与删除：远端来源的写入（upsert、发布换 id）与服务器确认的删除。

use std::collections::{HashMap, HashSet};
use tauri::{AppHandle, Emitter, State};

use crate::content::{preserve_local_sources, ClipboardEntry};
use crate::pending::delete_rows_for;
use crate::store::{
    collect_local_garbage, delete_entries_by_ids, history_path_for_key,
    mark_files_uploaded as store_mark_files_uploaded, placeholders, save_metadata,
    select_entries, select_entry, upsert_entry_row,
};
use crate::AppState;

use super::entry_contents_of;

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn upsert_remote_entry(
    app: AppHandle,
    state: State<'_, AppState>,
    entry: ClipboardEntry,
    available_file_ids: Vec<String>,
) -> Result<(), String> {
    upsert_remote_entries(app, state, vec![entry], available_file_ids)
}

/// Reconciling a fresh install can deliver hundreds of remote entries at once;
/// a single lock, save and event keeps that from locking up the windows.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn upsert_remote_entries(
    app: AppHandle,
    state: State<'_, AppState>,
    entries: Vec<ClipboardEntry>,
    available_file_ids: Vec<String>,
) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        // Existing rows' sources survive a remote overwrite: the remote copy
        // has no idea which local paths still hold the content here.
        let local_rows = if entries.is_empty() {
            HashMap::new()
        } else {
            state.with_database(&history_path, |connection| {
                let where_sql = format!("WHERE id IN ({})", placeholders(entries.len()));
                let values = entries
                    .iter()
                    .map(|entry| rusqlite::types::Value::Text(entry.id.clone()))
                    .collect::<Vec<_>>();
                select_entries(connection, &where_sql, "", &values)
            })?
            .into_iter()
            .map(|row| (row.id.clone(), row))
            .collect::<HashMap<_, _>>()
        };
        // The caller pre-queried which contents the server's pool holds
        // (POST /files/query). Those need no re-upload from this device, and
        // the availability rows make the state survive a restart.
        let available: HashSet<String> = entries
            .iter()
            .flat_map(entry_contents_of)
            .map(|(file_id, _)| file_id)
            .filter(|file_id| available_file_ids.contains(file_id))
            .collect();
        history.uploaded_files.extend(available.iter().cloned());
        let mut upserts = Vec::with_capacity(entries.len());
        for mut entry in entries {
            if let Some(local) = local_rows.get(&entry.id) {
                preserve_local_sources(&mut entry, local);
            }
            upserts.push(entry);
        }
        // One transaction: entry rows plus the availability rows. The rows go
        // in ascending created_at order, so within one millisecond the newest
        // insert gets the highest rowid and the created_ms DESC, rowid DESC
        // index yields a stable newest-first order.
        upserts.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        let available_vec = available.into_iter().collect::<Vec<_>>();
        state.with_database(&history_path, |connection| {
            let transaction = connection.transaction().map_err(|error| error.to_string())?;
            for entry in &upserts {
                upsert_entry_row(&transaction, entry)?;
            }
            store_mark_files_uploaded(&transaction, &available_vec);
            transaction.commit().map_err(|error| error.to_string())?;
            Ok(())
        })?;
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

/// Adopts the server's record for a locally captured entry: the local
/// content-hash id is swapped for the server-assigned one so local history,
/// the pending sets and every entryId command share the server's key space.
/// Returns false when the entry was deleted locally while the publish was in
/// flight — the caller must then delete the server row itself.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn apply_published_entry(
    app: AppHandle,
    state: State<'_, AppState>,
    local_entry_id: String,
    mut entry: ClipboardEntry,
) -> Result<bool, String> {
    {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        // A deletion can land while the publish is in flight — including of an
        // entry that was still unpublished. Without the local entry there is
        // nothing to adopt, and the caller must delete the server row.
        let Some(local) = state.with_database(&history_path, |connection| select_entry(connection, &local_entry_id))?
        else {
            return Ok(false);
        };
        preserve_local_sources(&mut entry, &local);
        // The WS echo may have inserted the server id before the publish
        // response arrived; drop both ids so only one row survives. The
        // response carries the server's timestamp, so the full response row
        // replaces both ids in one transaction — a pure id swap would leave a
        // stale timestamp behind.
        let entry_id = entry.id.clone();
        let adopted = entry.clone();
        state.with_database(&history_path, |connection| {
            let transaction = connection.transaction().map_err(|error| error.to_string())?;
            let removed = vec![local_entry_id.clone(), entry_id.clone()];
            delete_entries_by_ids(&transaction, &removed)?;
            delete_rows_for(&transaction, std::slice::from_ref(&local_entry_id))?;
            upsert_entry_row(&transaction, &adopted)?;
            save_metadata(&transaction, &history)?;
            transaction.commit().map_err(|error| error.to_string())?;
            Ok(())
        })?;
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())?;
    Ok(true)
}

/// Applies a server-confirmed deletion: drops the entry and its queue row,
/// then frees the blobs it referenced.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn remove_remote_entry(app: AppHandle, state: State<'_, AppState>, entry_id: String) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let path = history_path_for_key(&state.histories_dir, &history.active_history);
        state.with_database(&path, |connection| {
            let transaction = connection.transaction().map_err(|error| error.to_string())?;
            delete_entries_by_ids(&transaction, std::slice::from_ref(&entry_id))?;
            delete_rows_for(&transaction, std::slice::from_ref(&entry_id))?;
            save_metadata(&transaction, &history)?;
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
