//! Sync-client views over the durable upload queue and the deletion
//! tombstones.

use serde::Serialize;
use tauri::State;

use crate::store::{
    acknowledge_pending_entry as acknowledge_queue_row, history_path_for_key, list_pending_rows,
    temp_entry_id,
};
use crate::{save_active_history, AppState};

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
        save_active_history(&state, &history)?;
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
    let rows = list_pending_rows(&history_path_for_key(
        &state.histories_dir,
        &history.active_history,
    ))?;
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
    acknowledge_queue_row(
        &history_path_for_key(&state.histories_dir, &history.active_history),
        seq,
    )?;
    Ok(())
}
