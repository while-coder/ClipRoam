//! Server-selected history cache reads and missing-record checks.
use std::collections::{HashMap, HashSet};
use rusqlite::{params_from_iter, types::Value};
use tauri::State;

use crate::content::{lightweight_entry, refresh_summary, ClipboardEntry, SummaryContext};
use crate::file::{blob_ids_on_disk, history_stored_ids};
use crate::store::select_entries;
use crate::utils::placeholders;
use crate::AppState;

/// Read only the server page's identities; filtering, paging and total stay server-side.
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn get_cached_entries_for_display(
    state: State<'_, AppState>,
    entry_ids: Vec<String>,
) -> Result<Vec<ClipboardEntry>, String> {
    if entry_ids.is_empty() { return Ok(Vec::new()); }
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    let stored = history_stored_ids(&state, &mut history)?;
    let cache_dir = state.active_cache_dir(&history)?;
    let blobs = blob_ids_on_disk(&cache_dir);
    let context = SummaryContext { stored: &stored, blobs: &blobs, cache_dir: &cache_dir };
    let path = state.active_history_path(&history)?;
    let values = entry_ids.iter().cloned().map(Value::Text).collect::<Vec<_>>();
    let entries = state.with_database(&path, |connection| {
        select_entries(connection, &format!("WHERE id IN ({})", placeholders(entry_ids.len())), "", &values)
    })?;
    let mut by_id = entries.into_iter().map(|entry| (entry.id.clone(), entry)).collect::<HashMap<_, _>>();
    Ok(entry_ids.into_iter().filter_map(|id| {
        let mut entry = by_id.remove(&id)?;
        refresh_summary(&mut entry, &context);
        Some(lightweight_entry(&entry))
    }).collect())
}

/// Of the given ids, those the local history does not store — the remote side
/// of the sync reconcile's manifest diff. The manifest page is a few dozen
/// ids, so the membership test runs inside SQLite instead of hauling every
/// local id across the IPC boundary. Input order is preserved.
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn find_unknown_entry_ids(
    state: State<'_, AppState>,
    entry_ids: Vec<String>,
) -> Result<Vec<String>, String> {
    if entry_ids.is_empty() {
        return Ok(Vec::new());
    }
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let path = state.active_history_path(&history)?;
    state.with_database(&path, |connection| {
        let marks = placeholders(entry_ids.len());
        let sql = format!("SELECT id FROM entries WHERE id IN ({marks})");
        let mut statement = connection.prepare(&sql).map_err(|error| error.to_string())?;
        let values = entry_ids
            .iter()
            .map(|id| Value::Text(id.clone()))
            .collect::<Vec<_>>();
        let present = statement
            .query_map(params_from_iter(values), |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(entry_ids.into_iter().filter(|id| !present.contains(id)).collect())
    })
}

/// Full cached details for previews and clipboard actions.
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn get_entry(state: State<'_, AppState>, entry_id: String) -> Result<ClipboardEntry, String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    let stored = history_stored_ids(&state, &mut history)?;
    let cache_dir = state.active_cache_dir(&history)?;
    let blobs = blob_ids_on_disk(&cache_dir);
    let context = SummaryContext { stored: &stored, blobs: &blobs, cache_dir: &cache_dir };
    let path = state.active_history_path(&history)?;
    let mut entry = state
        .with_database(&path, |connection| {
            select_entries(connection, "WHERE id = ?", "", &[Value::Text(entry_id.clone())])
        })?
        .into_iter()
        .next()
        .ok_or_else(|| "剪贴板记录不存在".to_string())?;
    refresh_summary(&mut entry, &context);
    Ok(entry)
}
