//! History queries and lifecycle commands: listing, reading, refreshing,
//! deleting. Entries live in SQLite; every read goes through SQL, and each
//! row's derived `summary` is recomputed just before it leaves the backend.

use std::collections::{HashMap, HashSet};
use rusqlite::types::Value;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::clipboard::capture::lightweight_entry;
use crate::content::{refresh_summary, tree_contents, ClipboardEntry};
use crate::store::{
    collect_local_garbage, count_entries, history_path_for_key, newest_first_sql,
    refresh_entry_summary, select_all_entry_ids, select_entries, temp_entry_seq, HistoryData,
};
use crate::{active_cache_dir, flush_active_history, AppState};

/// Page size for `list_entries_manifest`; mirrors `PAGE_SIZE` in the frontend.
const MANIFEST_PAGE_SIZE: usize = 50;
/// Matches the fallback in the frontend's `deviceName` helper, so searching by
/// it finds the same entries in both places.
const UNKNOWN_DEVICE_LABEL: &str = "未知设备";
/// Upper bound for `list_upload_candidates`, so a huge library cannot turn a
/// settings toggle into an unbounded publish run.
const UPLOAD_CANDIDATE_LIMIT: usize = 500;

/// Filters for `list_entries_manifest`, mirroring `GET /entries/manifest` on
/// the server: keyword, kind and time range, then a page of the matches. An
/// absent `page` returns every match — used where the whole history is needed.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntriesManifestFilter {
    #[serde(default)]
    query: String,
    #[serde(default)]
    kind: String,
    start: Option<i64>,
    end: Option<i64>,
    page: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntriesManifestPage {
    total: usize,
    entries: Vec<ClipboardEntry>,
}

fn manifest_matches(
    entry: &ClipboardEntry,
    filter: &EntriesManifestFilter,
    device_names: &HashMap<String, String>,
    needle: &str,
) -> bool {
    if filter.kind != "all" && entry.kind != filter.kind {
        return false;
    }
    if !needle.is_empty() {
        let device_label = device_names
            .get(&entry.source_device_id)
            .map(String::as_str)
            .unwrap_or(UNKNOWN_DEVICE_LABEL);
        let matched = entry.content.to_lowercase().contains(needle)
            || device_label.to_lowercase().contains(needle);
        if !matched {
            return false;
        }
    }
    if filter.start.is_some() || filter.end.is_some() {
        let Ok(created_at) = DateTime::parse_from_rfc3339(&entry.created_at) else {
            return false;
        };
        let created_at = created_at.timestamp_millis();
        if filter.start.is_some_and(|start| created_at < start)
            || filter.end.is_some_and(|end| created_at > end)
        {
            return false;
        }
    }
    true
}

fn manifest_page(
    entries: &[ClipboardEntry],
    filter: &EntriesManifestFilter,
    device_names: &HashMap<String, String>,
) -> EntriesManifestPage {
    let needle = filter.query.trim().to_lowercase();
    let matched = entries
        .iter()
        .filter(|entry| manifest_matches(entry, filter, device_names, &needle))
        .collect::<Vec<_>>();
    let total = matched.len();
    let entries = match filter.page {
        Some(page) => {
            let skip = page.saturating_sub(1) * MANIFEST_PAGE_SIZE;
            matched
                .into_iter()
                .skip(skip)
                .take(MANIFEST_PAGE_SIZE)
                .map(lightweight_entry)
                .collect()
        }
        None => matched.into_iter().map(lightweight_entry).collect(),
    };
    EntriesManifestPage { total, entries }
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn list_entries_manifest(
    state: State<'_, AppState>,
    filter: EntriesManifestFilter,
    device_names: HashMap<String, String>,
) -> Result<EntriesManifestPage, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    Ok(manifest_page(history.active_entries(), &filter, &device_names))
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn list_entries_query(
    state: State<'_, AppState>,
    entry_ids: Vec<String>,
) -> Result<Vec<ClipboardEntry>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    Ok(entry_ids
        .iter()
        .filter_map(|entry_id| history.find(entry_id).map(lightweight_entry))
        .collect())
}

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
    flush_active_history(state, history, &[])?;
    // Dropping references is what frees disk space, so the sweep runs here.
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    let _ = state.with_database(&path, |connection| {
        collect_local_garbage(connection, &state.histories_dir, history)
    });
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
