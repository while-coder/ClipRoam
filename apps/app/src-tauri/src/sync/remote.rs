//! Applying remote (server-originated) entry and file-availability changes to
//! the local history.

use serde::Serialize;
use std::collections::HashSet;
use tauri::{AppHandle, Emitter, State};

use crate::content::{preserve_local_sources, ClipboardEntry};
use crate::store::{
    acknowledge_pending_entry as acknowledge_queue_row, collect_local_garbage, history_path_for_key,
    list_pending_rows, mark_files_uploaded as store_mark_files_uploaded, refresh_entry_summary,
    temp_entry_id,
};
use crate::history::{entry_contents_of, entry_references};
use crate::{active_cache_dir, flush_active_history, AppState};

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
        let cache_dir = active_cache_dir(&state, &history);
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        let mut upserted_ids = Vec::with_capacity(entries.len());
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
        {
            let slot = history.active_entries_mut();
            for mut entry in entries {
                if let Some(local) = slot.iter().find(|item| item.id == entry.id) {
                    preserve_local_sources(&mut entry, local);
                }
                let entry_id = entry.id.clone();
                slot.retain(|item| item.id != entry.id);
                slot.push(entry);
                upserted_ids.push(entry_id);
            }
            slot.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        }
        for entry_id in &upserted_ids {
            refresh_entry_summary(&mut history, entry_id, &cache_dir);
        }
        // The rows carry the full payload (tree, sources, rich text), so a
        // remote update lands in the same write as the rest of the flush.
        let upserts = upserted_ids
            .iter()
            .filter_map(|entry_id| history.find(entry_id))
            .collect::<Vec<_>>();
        flush_active_history(&state, &history, &upserts)?;
        state.with_database(&history_path, |connection| {
            let available_vec = available.iter().cloned().collect::<Vec<_>>();
            store_mark_files_uploaded(connection, &available_vec);
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
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        if history.pending_deletions.remove(&local_entry_id) {
            flush_active_history(&state, &history, &[])?;
            return Ok(false);
        }
        // A deletion can land while the publish is in flight — including of an
        // entry that was still unpublished. Without the local entry there is
        // nothing to adopt, and the caller must delete the server row.
        if history.find(&local_entry_id).is_none() {
            return Ok(false);
        }
        let cache_dir = active_cache_dir(&state, &history);
        if let Some(local) = history.find(&local_entry_id) {
            preserve_local_sources(&mut entry, local);
        }
        // The WS echo may have inserted the server id before the publish
        // response arrived; drop both keys so only one row survives.
        let entries = history.active_entries_mut();
        entries.retain(|item| item.id != local_entry_id && item.id != entry.id);
        let entry_id = entry.id.clone();
        entries.push(entry);
        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        refresh_entry_summary(&mut history, &entry_id, &cache_dir);
        // The id changed, so the upsert writes a fresh row and the mark-sweep
        // deletes the old one.
        let upserts = history.find(&entry_id).into_iter().collect::<Vec<_>>();
        flush_active_history(&state, &history, &upserts)?;
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())?;
    Ok(true)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn mark_files_uploaded(
    app: AppHandle,
    state: State<'_, AppState>,
    entry_id: String,
    file_ids: Vec<String>,
) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let cache_dir = active_cache_dir(&state, &history);
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        let uploaded = file_ids.into_iter().collect::<Vec<_>>();
        history.uploaded_files.extend(uploaded.iter().cloned());
        state.with_database(&history_path, |connection| {
            store_mark_files_uploaded(connection, &uploaded);
            Ok(())
        })?;
        refresh_entry_summary(&mut history, &entry_id, &cache_dir);
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

/// Server storage is content-addressed, so another device can finish uploading
/// a file after this entry was already received locally. Update every local
/// entry that references the now-available content.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn mark_file_available(
    app: AppHandle,
    state: State<'_, AppState>,
    file_id: String,
) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let cache_dir = active_cache_dir(&state, &history);
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        if history.uploaded_files.contains(&file_id) {
            return Ok(());
        }
        let changed_ids = history
            .active_entries()
            .iter()
            .filter(|entry| entry_references(entry, &file_id))
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        history.uploaded_files.insert(file_id.clone());
        state.with_database(&history_path, |connection| {
            store_mark_files_uploaded(connection, std::slice::from_ref(&file_id));
            Ok(())
        })?;
        for entry_id in &changed_ids {
            refresh_entry_summary(&mut history, entry_id, &cache_dir);
        }
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

/// Applies a server-confirmed deletion without creating a new local tombstone.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn remove_remote_entry(app: AppHandle, state: State<'_, AppState>, entry_id: String) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        history.active_entries_mut().retain(|entry| entry.id != entry_id);
        flush_active_history(&state, &history, &[])?;
        let path = history_path_for_key(&state.histories_dir, &history.active_history);
        let _ = state.with_database(&path, |connection| {
            collect_local_garbage(connection, &state.histories_dir, &mut history)
        });
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// 同步客户端视图：上传队列与删除墓碑
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) fn list_pending_deletions(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let mut pending = history.pending_deletions.iter().cloned().collect::<Vec<_>>();
    pending.sort();
    Ok(pending)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn acknowledge_entry_deletion(state: State<'_, AppState>, entry_id: String) -> Result<(), String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    if history.pending_deletions.remove(&entry_id) {
        flush_active_history(&state, &history, &[])?;
    }
    Ok(())
}

/// One durable upload-queue row for the sync client, enriched with the local
/// entry state the publish flow needs.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingQueueRowView {
    seq: i64,
    kind: String,
    content: String,
    extra: serde_json::Value,
    created_at: String,
    /// The temporary id the local entry carries until the server's is adopted.
    local_id: String,
    /// False once the entry was adopted, evicted or deleted — the row only
    /// needs acknowledging then.
    exists: bool,
    /// Files entries are publishable only once every content id is resolved.
    ready: bool,
}

#[tauri::command]
pub(crate) fn list_pending_entries(state: State<'_, AppState>) -> Result<Vec<PendingQueueRowView>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let rows = state.with_database(
        &history_path_for_key(&state.histories_dir, &history.active_history),
        |connection| list_pending_rows(connection),
    )?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let local_id = temp_entry_id(row.seq);
            let entry = history.find(&local_id);
            let ready = match entry {
                // Same rule as the hash-resume list (`hashing_pending`): any
                // unresolved source file means the payload is not final yet.
                Some(entry) if entry.kind == "files" => !entry.hashing_pending(),
                Some(_) => true,
                None => false,
            };
            let extra = serde_json::from_str(&row.extra).unwrap_or_else(|_| {
                serde_json::json!({ "html": null, "rtf": null, "fileInfo": null, "imageInfo": null })
            });
            PendingQueueRowView {
                seq: row.seq,
                kind: row.kind,
                content: row.content,
                extra,
                created_at: row.created_at,
                exists: entry.is_some(),
                ready,
                local_id,
            }
        })
        .collect())
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn acknowledge_pending_entry(state: State<'_, AppState>, seq: i64) -> Result<(), String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    state.with_database(&path, |connection| acknowledge_queue_row(connection, seq))?;
    Ok(())
}
