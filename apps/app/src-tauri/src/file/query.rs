//! 文件内容相关的查询命令：历史引用的内容 id，以及候选上传条目。

use std::collections::HashSet;

use tauri::State;

use super::cache::history_file_ids as cache_history_file_ids;
use super::store::history_stored_ids;
use crate::content::{refresh_summary, ClipboardEntry, SummaryContext};
use crate::entry::lightweight_entry;
use crate::store::{history_path_for_key, select_entries};
use crate::{active_cache_dir, AppState};

/// Upper bound for `list_upload_candidates`, so a huge library cannot turn a
/// settings toggle into an unbounded publish run.
const UPLOAD_CANDIDATE_LIMIT: usize = 500;

/// Every content id the durable history references, derived from the entries'
/// extras Rust-side so the whole history never crosses the IPC boundary. The
/// derived set is cached on the history and invalidated by any `entries`
/// write, so repeated calls in a refresh burst re-parse nothing.
pub(crate) fn derived_history_file_ids(
    state: &AppState,
    history: &mut crate::store::HistoryData,
) -> Result<HashSet<String>, String> {
    if history.file_ids.is_none() {
        let path = history_path_for_key(&state.histories_dir, &history.active_history);
        let ids = state.with_database(&path, |connection| Ok(cache_history_file_ids(connection)))?;
        history.file_ids = Some(ids);
    }
    Ok(history
        .file_ids
        .as_ref()
        .expect("the set was derived above when absent")
        .clone())
}

/// Files/image entries fully hashed whose uploadable payload fits the
/// automatic-upload limit; drives the settings page's "upload now" run.
#[tauri::command(rename_all = "camelCase", async)]
pub(crate) fn list_upload_candidates(
    state: State<'_, AppState>,
    limit_bytes: u64,
) -> Result<Vec<ClipboardEntry>, String> {
    let mut history = state.history.lock().map_err(|error| error.to_string())?;
    let stored = history_stored_ids(&state, &mut history)?;
    let cache_dir = active_cache_dir(&state, &history);
    let blobs = super::blob_ids_on_disk(&cache_dir);
    let context = SummaryContext { stored: &stored, blobs: &blobs, cache_dir: &cache_dir };
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
        refresh_summary(entry, &context);
        if entry.summary.uploadable_size.is_some_and(|size| size < limit_bytes) {
            candidates.push(lightweight_entry(entry));
            if candidates.len() >= UPLOAD_CANDIDATE_LIMIT {
                break;
            }
        }
    }
    Ok(candidates)
}
