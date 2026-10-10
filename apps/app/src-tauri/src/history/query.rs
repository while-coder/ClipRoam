//! Server-selected history cache reads and revision checks.
use std::collections::HashMap;
use rusqlite::{params_from_iter, types::Value, Connection};
use serde::Deserialize;
use tauri::State;

use crate::content::{lightweight_entry, refresh_summary, ClipboardEntry, SummaryContext};
use crate::file::{blob_ids_on_disk, store::stored_file_ids};
use crate::store::{select_entries};
use crate::utils::placeholders;
use crate::AppState;

/// Read only the server page's identities; filtering, paging and total stay server-side.
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn get_cached_entries_for_display(
    state: State<'_, AppState>,
    entry_ids: Vec<String>,
    session_id: String,
) -> Result<Vec<ClipboardEntry>, String> {
    let account = state.account(&session_id)?;
    if entry_ids.is_empty() { return Ok(Vec::new()); }
    let stored = account.with_database(|connection| stored_file_ids(connection))?;
    let cache_dir = account.cache_dir.clone();
    let blobs = blob_ids_on_disk(&cache_dir);
    let context = SummaryContext { stored: &stored, blobs: &blobs, cache_dir: &cache_dir };
    let values = entry_ids.iter().cloned().map(Value::Text).collect::<Vec<_>>();
    let entries = account.with_database(|connection| {
        select_entries(connection, &format!("WHERE id IN ({})", placeholders(entry_ids.len())), "", &values)
    })?;
    let mut by_id = entries.into_iter().map(|entry| (entry.id.clone(), entry)).collect::<HashMap<_, _>>();
    Ok(entry_ids.into_iter().filter_map(|id| {
        let mut entry = by_id.remove(&id)?;
        refresh_summary(&mut entry, &context);
        Some(lightweight_entry(&entry))
    }).collect())
}

#[derive(Deserialize)]
pub(crate) struct ClipboardManifestEntry {
    id: String,
    version: u64,
}

/// Compare only this manifest's revisions, preserving server page order.
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn find_stale_entry_ids(
    state: State<'_, AppState>,
    manifest: Vec<ClipboardManifestEntry>,
    session_id: String,
) -> Result<Vec<String>, String> {
    let account = state.account(&session_id)?;
    if manifest.is_empty() {
        return Ok(Vec::new());
    }
    account.with_database(|connection| stale_entry_ids(connection, manifest))
}

fn stale_entry_ids(connection: &Connection, manifest: Vec<ClipboardManifestEntry>) -> Result<Vec<String>, String> {
    if manifest.is_empty() { return Ok(Vec::new()); }
    let marks = placeholders(manifest.len());
    let sql = format!("SELECT id, version FROM entries WHERE id IN ({marks})");
    let mut statement = connection.prepare(&sql).map_err(|error| error.to_string())?;
    let values = manifest.iter().map(|entry| Value::Text(entry.id.clone())).collect::<Vec<_>>();
    let present = statement
        .query_map(params_from_iter(values), |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)))
        .map_err(|error| error.to_string())?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(manifest.into_iter()
        .filter(|entry| present.get(&entry.id).is_none_or(|version| *version < entry.version))
        .map(|entry| entry.id).collect())
}

/// Full cached details for previews and clipboard actions.
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn get_entry(state: State<'_, AppState>, entry_id: String, session_id: String) -> Result<ClipboardEntry, String> {
    let account = state.account(&session_id)?;
    let stored = account.with_database(|connection| stored_file_ids(connection))?;
    let cache_dir = account.cache_dir.clone();
    let blobs = blob_ids_on_disk(&cache_dir);
    let context = SummaryContext { stored: &stored, blobs: &blobs, cache_dir: &cache_dir };
    let mut entry = account.with_database(|connection| {
            select_entries(connection, "WHERE id = ?", "", &[Value::Text(entry_id.clone())])
        })?
        .into_iter()
        .next()
        .ok_or_else(|| "剪贴板记录不存在".to_string())?;
    refresh_summary(&mut entry, &context);
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_only_fetches_missing_and_newer_revisions_in_page_order() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE entries (id TEXT PRIMARY KEY, version INTEGER NOT NULL);
            INSERT INTO entries VALUES ('legacy', 0), ('same', 2), ('changed', 1), ('ahead', 3);").unwrap();
        let manifest = [("changed", 2), ("same", 2), ("missing", 1), ("legacy", 1), ("ahead", 2)]
            .into_iter().map(|(id, version)| ClipboardManifestEntry { id: id.into(), version }).collect();
        assert_eq!(stale_entry_ids(&connection, manifest).unwrap(), ["changed", "missing", "legacy"]);
        assert!(stale_entry_ids(&connection, Vec::new()).unwrap().is_empty());
    }
}
