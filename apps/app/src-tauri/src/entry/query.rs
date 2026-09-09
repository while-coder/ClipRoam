//! 条目读取：manifest / query / ids / 候选上传 / 单条读取。
//! Entries live in SQLite; every read goes through SQL, and each row's derived
//! `summary` is recomputed just before it leaves the backend.

use std::collections::HashMap;
use rusqlite::types::Value;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::content::{refresh_summary, ClipboardEntry};
use crate::store::{
    count_entries, history_file_ids as store_history_file_ids, history_path_for_key,
    newest_first_sql, select_all_entry_ids, select_entries,
};
use crate::{active_cache_dir, AppState};

use super::lightweight_entry;

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

/// Escapes LIKE wildcards so the keyword matches literally, like the
/// frontend's `String.includes`.
fn escape_like(needle: &str) -> String {
    let mut escaped = String::with_capacity(needle.len());
    for character in needle.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// Builds the WHERE clause and parameters for the manifest filters. Keyword
/// matching mirrors the frontend's `clientManifest`: entry content, or the
/// device label with the same "未知设备" fallback. SQLite's `lower()` folds
/// ASCII only — identical behaviour for CJK, narrower for accented Latin.
fn manifest_query(
    filter: &EntriesManifestFilter,
    device_names: &HashMap<String, String>,
    needle: &str,
) -> (String, Vec<Value>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut values: Vec<Value> = Vec::new();
    if filter.kind != "all" {
        clauses.push("kind = ?".to_string());
        values.push(Value::Text(filter.kind.clone()));
    }
    if !needle.is_empty() {
        let matching_ids = device_names
            .iter()
            .filter(|(_, name)| name.to_lowercase().contains(needle))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let unknown_matches = UNKNOWN_DEVICE_LABEL.contains(needle);
        // With no known devices every entry falls into the fallback group, so
        // the keyword test passes for all of them.
        if !(unknown_matches && device_names.is_empty()) {
            let mut alternatives = vec!["LOWER(content) LIKE ? ESCAPE '\\'".to_string()];
            values.push(Value::Text(format!("%{}%", escape_like(needle))));
            if !matching_ids.is_empty() {
                alternatives.push(format!(
                    "source_device_id IN ({})",
                    crate::store::placeholders(matching_ids.len())
                ));
                values.extend(matching_ids.into_iter().map(Value::Text));
            }
            if unknown_matches {
                alternatives.push(format!(
                    "source_device_id NOT IN ({})",
                    crate::store::placeholders(device_names.len())
                ));
                values.extend(device_names.keys().map(|id| Value::Text(id.clone())));
            }
            clauses.push(format!("({})", alternatives.join(" OR ")));
        }
    }
    if let Some(start) = filter.start {
        clauses.push("created_ms >= ?".to_string());
        values.push(Value::Integer(start));
    }
    if let Some(end) = filter.end {
        clauses.push("created_ms <= ?".to_string());
        values.push(Value::Integer(end));
    }
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    (where_sql, values)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn list_entries_manifest(
    state: State<'_, AppState>,
    filter: EntriesManifestFilter,
    device_names: HashMap<String, String>,
) -> Result<EntriesManifestPage, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let cache_dir = active_cache_dir(&state, &history);
    let needle = filter.query.trim().to_lowercase();
    let (where_sql, values) = manifest_query(&filter, &device_names, &needle);
    let (limit, offset) = match filter.page {
        Some(page) => {
            let offset = page.saturating_sub(1) * MANIFEST_PAGE_SIZE;
            (Some(MANIFEST_PAGE_SIZE), offset)
        }
        None => (None, 0),
    };
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    // Count and page come out of one pass over the same connection so a
    // concurrent capture cannot slip between them.
    let (total, entries) = state.with_database(&path, |connection| {
        let total = count_entries(connection, &where_sql, &values)?;
        let entries = select_entries(connection, &where_sql, &newest_first_sql(limit, offset), &values)?;
        Ok((total, entries))
    })?;
    let mut entries = entries;
    for entry in &mut entries {
        refresh_summary(entry, &history.cached_files, &cache_dir);
    }
    Ok(EntriesManifestPage {
        total,
        entries: entries.iter().map(lightweight_entry).collect(),
    })
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn list_entries_query(
    state: State<'_, AppState>,
    entry_ids: Vec<String>,
) -> Result<Vec<ClipboardEntry>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let cache_dir = active_cache_dir(&state, &history);
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    let mut found = if entry_ids.is_empty() {
        HashMap::new()
    } else {
        state.with_database(&path, |connection| {
            let where_sql = format!("WHERE id IN ({})", crate::store::placeholders(entry_ids.len()));
            let values = entry_ids
                .iter()
                .map(|id| Value::Text(id.clone()))
                .collect::<Vec<_>>();
            select_entries(connection, &where_sql, "", &values)
        })?
        .into_iter()
        .map(|entry| (entry.id.clone(), entry))
        .collect::<HashMap<_, _>>()
    };
    // The caller's id order is preserved; missing ids are simply absent.
    Ok(entry_ids
        .iter()
        .filter_map(|entry_id| {
            let mut entry = found.remove(entry_id)?;
            refresh_summary(&mut entry, &history.cached_files, &cache_dir);
            Some(lightweight_entry(&entry))
        })
        .collect())
}

/// Every stored entry id, newest first — the local side of the sync
/// reconcile's manifest diff and the "clear history" total.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn list_entry_ids(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    state.with_database(&path, |connection| select_all_entry_ids(connection))
}

/// Total entries across every filter — the clear-history affordance.
#[tauri::command]
pub(crate) fn total_entry_count(state: State<'_, AppState>) -> Result<usize, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    state.with_database(&path, |connection| count_entries(connection, "", &[]))
}

/// Every content id the durable history references, derived from the entries'
/// extras Rust-side so the whole history never crosses the IPC boundary. The
/// frontend asks the pool (`/files/query`) which of these it holds to render
/// upload status; nothing is persisted locally.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn history_file_ids(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    state.with_database(&path, |connection| Ok(store_history_file_ids(connection)))
}

/// Files/image entries fully hashed whose uploadable payload fits the
/// automatic-upload limit; drives the settings page's "upload now" run.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn list_upload_candidates(
    state: State<'_, AppState>,
    limit_bytes: u64,
) -> Result<Vec<ClipboardEntry>, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let cache_dir = active_cache_dir(&state, &history);
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    // Files entries still hashing carry an unresolved `"fileId":null` source;
    // images have no hashing step.
    let mut entries = state.with_database(&path, |connection| {
        select_entries(
            connection,
            "WHERE kind IN ('files', 'image') AND (kind = 'image' OR sources NOT LIKE '%\"fileId\":null%')",
            "",
            &[],
        )
    })?;
    let mut candidates = Vec::new();
    for entry in &mut entries {
        refresh_summary(entry, &history.cached_files, &cache_dir);
        if entry.summary.uploadable_size.is_some_and(|size| size < limit_bytes) {
            candidates.push(lightweight_entry(entry));
            if candidates.len() >= UPLOAD_CANDIDATE_LIMIT {
                break;
            }
        }
    }
    Ok(candidates)
}

/// The full entry, tree included — used when publishing to the server.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn get_entry(state: State<'_, AppState>, entry_id: String) -> Result<ClipboardEntry, String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    let cache_dir = active_cache_dir(&state, &history);
    let path = history_path_for_key(&state.histories_dir, &history.active_history);
    let mut entry = state
        .with_database(&path, |connection| {
            select_entries(connection, "WHERE id = ?", "", &[Value::Text(entry_id.clone())])
        })?
        .into_iter()
        .next()
        .ok_or_else(|| "剪贴板记录不存在".to_string())?;
    refresh_summary(&mut entry, &history.cached_files, &cache_dir);
    Ok(entry)
}

/// Summaries are recomputed from the availability sets every time an entry is
/// read, so this is a pure re-read trigger for the frontend; it exists to
/// keep the invoke shape stable across the read-model change.
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn refresh_entry(state: State<'_, AppState>, entry_id: String) -> Result<(), String> {
    let _ = (state, entry_id);
    Ok(())
}
