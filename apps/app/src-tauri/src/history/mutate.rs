//! 历史详情补查写入缓存，以及服务器确认的删除。

use tauri::{AppHandle, Emitter, State};

use crate::content::ClipboardEntry;
use crate::file::collect_local_garbage;
use crate::store::{delete_entries_by_ids, upsert_entry_row, with_transaction};
use crate::AppState;

/// Reconciling a fresh install can deliver hundreds of server entries at once;
/// one archive-bound transaction stores the queried details without invalidating
/// the history view that is already refreshing them.
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn upsert_server_entries(
    state: State<'_, AppState>,
    entries: Vec<ClipboardEntry>,
    session_id: String,
) -> Result<(), String> {
    let account = state.account(&session_id)?;
    if entries.is_empty() {
        return Ok(());
    }
    {
        // The rows go in ascending created_at order, so within one millisecond
        // the newest insert gets the highest rowid and the created_ms DESC,
        // rowid DESC index yields a stable newest-first order.
        let mut upserts = entries;
        upserts.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        account.with_database(|connection| {
            with_transaction(connection, |transaction| {
                for entry in &upserts {
                    upsert_entry_row(transaction, entry)?;
                }
                Ok(())
            })
        })?;
    }
    Ok(())
}

/// Applies a server-confirmed deletion: drops the entry, then frees the blobs
/// it referenced.
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn remove_server_entry(app: AppHandle, state: State<'_, AppState>, entry_id: String, session_id: String) -> Result<(), String> {
    let account = state.account(&session_id)?;
    {
        // Serialize garbage collection with local captures; the target archive
        // still comes exclusively from the caller's context, never the active one.
        let _history = state.history.lock().map_err(|error| error.to_string())?;
        account.with_database(|connection| {
            with_transaction(connection, |transaction| {
                delete_entries_by_ids(transaction, std::slice::from_ref(&entry_id))?;
                Ok(())
            })
        })?;
        // Dropping references is what frees disk space, so the sweep runs here.
        let cache_dir = account.cache_dir.clone();
        let _ = account.with_database(|connection| {
            collect_local_garbage(connection, &cache_dir)
        });
    }
    app.emit("cliproam://history-changed", ())
        .map_err(|error| error.to_string())
}
