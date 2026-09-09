//! Applying remote (server-originated) entry and file-availability changes to
//! the local history.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use tauri::{AppHandle, Emitter, State};

use crate::content::{preserve_local_sources, ClipboardEntry, ClipboardEntryExtra};
use crate::store::{
    acknowledge_pending_entry as acknowledge_queue_row, collect_local_garbage,
    delete_entries_by_ids, delete_queue_rows_for, history_path_for_key, list_pending_rows,
    mark_files_uploaded as store_mark_files_uploaded, placeholders, save_metadata,
    select_entries, select_entry, temp_entry_id, upsert_entry_row,
};
use crate::history::entry_contents_of;
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
            delete_queue_rows_for(&transaction, std::slice::from_ref(&local_entry_id))?;
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
            delete_queue_rows_for(&transaction, std::slice::from_ref(&entry_id))?;
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

// ---------------------------------------------------------------------------
// 同步客户端视图：上传队列
// ---------------------------------------------------------------------------

/// One publishable upload-queue row for the sync client. The payload comes
/// from the local entry after content resolution, not from the raw row — a
/// files row keeps its capture-time tree (`f: ""`) until the drain resolves
/// it, so the two extras are deliberately not the same.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingQueueRowView {
    seq: i64,
    kind: String,
    content: String,
    extra: serde_json::Value,
}

/// The oldest publishable queue row, or `None` when the queue is empty or its
/// head cannot proceed yet. Rows whose entry vanished while waiting — adopted,
/// deduped or deleted — are acknowledged on the way, so the caller never sees
/// one. Strict order: a files entry whose content ids still resolve right
/// here blocks the tail, and the next `entry-created` event restarts the
/// drain afterwards.
#[tauri::command]
pub(crate) fn next_pending_entry(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<PendingQueueRowView>, String> {
    let rows = {
        let history = state.history.lock().map_err(|error| error.to_string())?;
        let path = history_path_for_key(&state.histories_dir, &history.active_history);
        state.with_database(&path, |connection| list_pending_rows(connection))?
    };
    for row in rows {
        let entry_id = temp_entry_id(row.seq);
        let mut entry = match read_entry(&state, &entry_id)? {
            Some(entry) => entry,
            None => {
                drop_stale_row(&state, row.seq)?;
                continue;
            }
        };
        if entry.kind == "files" {
            // Resolving a large tree can take a while, so the history lock is
            // released for it (`resolve_entry_files` re-locks per batch).
            crate::clipboard::hashing::resolve_entry_files(&app, &entry_id)?;
            entry = match read_entry(&state, &entry_id)? {
                Some(entry) => entry,
                // Deleted while the resolution ran: the row is stale now.
                None => {
                    drop_stale_row(&state, row.seq)?;
                    continue;
                }
            };
        }
        let extra = ClipboardEntryExtra::of(&entry).json()?;
        return Ok(Some(PendingQueueRowView {
            seq: row.seq,
            kind: entry.kind,
            content: entry.content,
            extra: serde_json::from_str(&extra).unwrap_or_else(|_| {
                serde_json::json!({ "html": null, "rtf": null, "fileInfo": null, "imageInfo": null })
            }),
        }));
    }
    Ok(None)
}

/// One entry row read under the history lock, which also pins the active
/// history the pooled connection is keyed by.
fn read_entry(state: &AppState, entry_id: &str) -> Result<Option<ClipboardEntry>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    state.with_database(&path, |connection| select_entry(connection, entry_id))
}

/// Removes a queue row whose entry is gone. Deleting it here — before any
/// later row is returned — keeps a repeated drain pass from republishing the
/// row onto a fresh server entry the caller would then have to delete again.
fn drop_stale_row(state: &AppState, seq: i64) -> Result<(), String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    state
        .with_database(&path, |connection| acknowledge_queue_row(connection, seq))
        .map(|_| ())
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn acknowledge_pending_entry(state: State<'_, AppState>, seq: i64) -> Result<(), String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    state
        .with_database(&path, |connection| acknowledge_queue_row(connection, seq))?;
    Ok(())
}
