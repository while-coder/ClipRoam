//! Applying remote (server-originated) entry and file-availability changes to
//! the local history.

use std::collections::HashSet;
use tauri::{AppHandle, Emitter, State};

use crate::content::{preserve_local_sources, ClipboardEntry};
use crate::store::{
    collect_local_garbage, history_path_for_key, mark_files_uploaded as store_mark_files_uploaded,
    open_history_database, refresh_entry_summary, trim_history, write_entry_data,
};
use crate::history::{entry_contents_of, entry_references};
use crate::{active_cache_dir, save_active_history, AppState};

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
    mut entry: ClipboardEntry,
    available_file_ids: Vec<String>,
) -> Result<(), String> {
    {
        let mut history = state.history.lock().map_err(|error| error.to_string())?;
        let cache_dir = active_cache_dir(&state, &history);
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
        // The caller pre-queried which contents the server's pool holds
        // (POST /files/query). Those need no re-upload from this device, and
        // the availability row makes the state survive a restart.
        let available: Vec<String> = entry_contents_of(&entry)
            .into_iter()
            .map(|(file_id, _)| file_id)
            .filter(|file_id| available_file_ids.contains(file_id))
            .collect();
        let entries = history.active_entries_mut();
        if let Some(local) = entries.iter().find(|item| item.id == entry.id) {
            preserve_local_sources(&mut entry, local);
        }
        entries.retain(|item| item.id != entry.id);
        let entry_id = entry.id.clone();
        entries.push(entry);
        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        trim_history(entries);
        refresh_entry_summary(&mut history, &entry_id, &cache_dir);
        save_active_history(&state, &history)?;
        // Existing rows keep their large data during the general history save;
        // a remote update replaces it explicitly here.
        if let Some(entry) = history.find(&entry_id) {
            let connection = open_history_database(&history_path)?;
            write_entry_data(&connection, entry)?;
            store_mark_files_uploaded(&connection, &available);
        }
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
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
            trim_history(slot);
        }
        for entry_id in &upserted_ids {
            refresh_entry_summary(&mut history, entry_id, &cache_dir);
        }
        save_active_history(&state, &history)?;
        // Existing rows keep their large data during the general history save;
        // a remote update replaces it explicitly here.
        let connection = open_history_database(&history_path)?;
        for entry_id in &upserted_ids {
            if let Some(entry) = history.find(entry_id) {
                write_entry_data(&connection, entry)?;
            }
        }
        let available_vec = available.into_iter().collect::<Vec<_>>();
        store_mark_files_uploaded(&connection, &available_vec);
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
            save_active_history(&state, &history)?;
            return Ok(false);
        }
        // A deletion can land while the publish is in flight — including of an
        // entry that was still unpublished. Without the local entry there is
        // nothing to adopt, and the caller must delete the server row.
        if history.find(&local_entry_id).is_none() {
            return Ok(false);
        }
        let cache_dir = active_cache_dir(&state, &history);
        let history_path = history_path_for_key(&state.histories_dir, &history.active_history);
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
        trim_history(entries);
        refresh_entry_summary(&mut history, &entry_id, &cache_dir);
        // The id changed, so the save's INSERT OR IGNORE writes a fresh row
        // and the mark-sweep deletes the old one.
        save_active_history(&state, &history)?;
        if let Some(entry) = history.find(&entry_id) {
            let connection = open_history_database(&history_path)?;
            write_entry_data(&connection, entry)?;
        }
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
        let connection = open_history_database(&history_path)?;
        store_mark_files_uploaded(&connection, &uploaded);
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
        let connection = open_history_database(&history_path)?;
        store_mark_files_uploaded(&connection, &[file_id]);
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
        save_active_history(&state, &history)?;
        let _ = collect_local_garbage(&state.histories_dir, &mut history);
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}
